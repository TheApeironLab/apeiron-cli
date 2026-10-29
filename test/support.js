export const binary = process.env.APEIRON_TEST_BIN || new URL('../target/debug/apeiron', import.meta.url).pathname;
export { post, until, localInstallation, DeploymentClient } from '../third_party/chentu/rust/test/support';
