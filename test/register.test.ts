import { test, expect } from "bun:test";
import { gatewayUrl, validTunnelRequest } from "../src/register";
test("gateway transport only allows HTTPS or explicit local development", () => {
  expect(gatewayUrl("https://gateway.apeironlab.cn").origin).toBe(
    "https://gateway.apeironlab.cn",
  );
  expect(gatewayUrl("http://localhost:8080").origin).toBe(
    "http://localhost:8080",
  );
  for (const value of [
    "http://evil.example",
    "https://user:secret@example.com",
    "https://example.com/path",
    "https://example.com/#token",
  ])
    expect(() => gatewayUrl(value)).toThrow();
});
test("tunnel accepts only bounded setup requests, never arbitrary target URLs", () => {
  const base = {
    type: "request",
    id: "aefcb762-56b7-4ff1-8969-5dd5a0895647",
    method: "GET",
    path: "api/config",
    body: "",
    headers: {},
  };
  expect(validTunnelRequest(base)).toBe(true);
  for (const path of [
    "http://169.254.169.254",
    "//evil.example",
    "../secret",
    "api/../secret",
    "api/config?redirect=1",
    "api/%2e%2e/secret",
  ])
    expect(validTunnelRequest({ ...base, path })).toBe(false);
  expect(validTunnelRequest({ ...base, method: "DELETE" })).toBe(false);
  expect(validTunnelRequest({ ...base, body: "x".repeat(16385) })).toBe(false);
});
