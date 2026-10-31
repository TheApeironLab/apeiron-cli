// Use Chentu's complete browser contract list against the embedded host.
process.env.CHENTU_EMBEDDED = '1';
await import('../third_party/chentu/rust/scripts/test-browser-suite.js');
