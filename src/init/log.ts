import { constants } from 'node:fs';
import { open } from 'node:fs/promises';
import { ConfigError } from './config';

// The server supplies only its own current run's path. No request parameter can
// select a file. Snapshot the size so a growing log cannot hold a request open.
export async function openDeploymentLog(path: string | undefined): Promise<{ stream: ReadableStream<Uint8Array>; size: number }> {
  if (!path) throw new ConfigError('安装日志尚未生成。', 404);
  let file;
  try { file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK); }
  catch { throw new ConfigError('安装日志不可读取。', 404); }
  const stat = await file.stat();
  if (!stat.isFile() || stat.nlink !== 1) { await file.close(); throw new ConfigError('安装日志不是普通文件。', 404); }
  const size = stat.size;
  let position = 0;
  let closed = false;
  const close = async () => { if (!closed) { closed = true; await file.close(); } };
  const stream = new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        if (position >= size) { await close(); controller.close(); return; }
        const chunk = Buffer.alloc(Math.min(64 * 1024, size - position));
        const { bytesRead } = await file.read(chunk, 0, chunk.length, position);
        if (!bytesRead) { await close(); controller.close(); return; }
        position += bytesRead;
        controller.enqueue(chunk.subarray(0, bytesRead));
      } catch (error) { await close(); controller.error(error); }
    },
    cancel: close,
  });
  return { stream, size };
}
