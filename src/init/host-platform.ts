// Kept self-contained so the browser and server use the same host policy.
// Matching deployment resources are checked separately before host changes.
export function supportsK3sHost(os: string, version: string, architecture: string): boolean {
  return os.toLowerCase() === 'ubuntu' && (
    /^22\.04(\.\d+)?$/.test(version) && ['amd64', 'x64', 'x86_64'].includes(architecture) ||
    /^24\.04(\.\d+)?$/.test(version) && ['arm64', 'aarch64'].includes(architecture)
  );
}
