import { execFile } from 'node:child_process';
import { promisify } from 'node:util';

// A separate read-only process: never replace the active deployment child.
export function watchK3dProgress(cluster: string, signal: AbortSignal, progress: (message: string) => void) {
  const controller = new AbortController();
  const stopped = AbortSignal.any([signal, controller.signal]);
  const execute = promisify(execFile);
  const started = Date.now();
  let busy = false;
  const seen = new Set<string>();
  const report = (message: string) => {
    if (!stopped.aborted && !seen.has(message)) { seen.add(message); progress(message); }
  };
  async function poll() {
    if (busy || stopped.aborted) return;
    busy = true;
    try {
      const { stdout } = await execute('docker', ['exec', `k3d-${cluster}-server-0`, 'kubectl',
        'get', 'pods', '-n', 'kube-system', '-o', 'json'], { signal: stopped, timeout: 8000, maxBuffer: 2 * 1024 * 1024 });
      const pods = JSON.parse(stdout).items;
      for (const pod of pods) {
        const name = pod.metadata.name;
        const statuses = [...(pod.status.initContainerStatuses ?? []), ...(pod.status.containerStatuses ?? [])];
        const ready = statuses.length && statuses.every((status: { ready: boolean }) => status.ready);
        const waiting = statuses.find((status: { state?: { waiting?: unknown } }) => status.state?.waiting)?.state.waiting;
        const warning = ['ImagePullBackOff', 'ErrImagePull', 'CrashLoopBackOff'].includes(waiting?.reason) || pod.status.phase === 'Failed';
        report(`[${warning ? 'WARNING' : 'INFO'}] K3d 基础服务：${name} — ${pod.status.phase === 'Succeeded' ? '已完成' : ready ? '已就绪' : waiting?.reason || pod.status.phase || '等待启动'}`);
      }
      const { stdout: eventOutput } = await execute('docker', ['exec', `k3d-${cluster}-server-0`, 'kubectl',
        'get', 'events', '-n', 'kube-system', '-o', 'json'], { signal: stopped, timeout: 8000, maxBuffer: 2 * 1024 * 1024 });
      for (const event of JSON.parse(eventOutput).items) {
        if (['Pulling', 'Pulled', 'Failed', 'FailedCreatePodSandBox', 'BackOff'].includes(event.reason)) {
          report(`[${event.type === 'Warning' ? 'WARNING' : 'INFO'}] K3d ${event.involvedObject.name}：${String(event.message).replace(/[\r\n]+/g, ' ').slice(0, 500)}`);
        }
      }
    } catch { /* The container/API may not exist yet; the deployment owns failures. */ }
    finally { busy = false; }
  }
  const timer = setInterval(() => {
    if (!stopped.aborted) progress(`K3d 准备仍在进行（已等待 ${Math.floor((Date.now() - started) / 1000)} 秒），正在检查基础服务状态。`);
    void poll();
  }, 15_000);
  timer.unref();
  return () => { clearInterval(timer); controller.abort(); };
}
