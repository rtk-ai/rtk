import test from "node:test";
import assert from "node:assert/strict";
import {
  compactDiscordUrl,
  normalizeDiscordChannelUrl,
  validateDiscordChannelUrl
} from "../src/shared/url.js";

test("accepts server channel Discord URLs", () => {
  const result = validateDiscordChannelUrl("https://discord.com/channels/123/456");
  assert.equal(result.ok, true);
  assert.equal(result.url, "https://discord.com/channels/123/456");
  assert.equal(result.scope, "server");
});

test("accepts direct message Discord URLs", () => {
  const result = validateDiscordChannelUrl("https://discord.com/channels/@me/456");
  assert.equal(result.ok, true);
  assert.equal(result.url, "https://discord.com/channels/@me/456");
  assert.equal(result.scope, "dm");
});

test("normalizes trailing slash and search params", () => {
  assert.equal(
    normalizeDiscordChannelUrl("https://discord.com/channels/123/456/?jump=999"),
    "https://discord.com/channels/123/456"
  );
});

test("rejects non-discord hosts", () => {
  const result = validateDiscordChannelUrl("https://example.com/channels/123/456");
  assert.equal(result.ok, false);
  assert.match(result.error, /discord\.com/);
});

test("rejects non-channel Discord URLs", () => {
  const result = validateDiscordChannelUrl("https://discord.com/app");
  assert.equal(result.ok, false);
  assert.match(result.error, /channels/);
});

test("rejects non-numeric server and channel ids", () => {
  assert.equal(validateDiscordChannelUrl("https://discord.com/channels/foo/456").ok, false);
  assert.equal(validateDiscordChannelUrl("https://discord.com/channels/123/bar").ok, false);
  assert.equal(validateDiscordChannelUrl("https://discord.com/channels/@me/not-a-snowflake").ok, false);
});

test("rejects non-https discord URLs", () => {
  const result = validateDiscordChannelUrl("http://discord.com/channels/123/456");
  assert.equal(result.ok, false);
});

test("rejects discord host spoofing", () => {
  const result = validateDiscordChannelUrl("https://discord.com.evil.test/channels/123/456");
  assert.equal(result.ok, false);
});

test("returns compact display labels", () => {
  assert.equal(compactDiscordUrl("https://discord.com/channels/123/456"), "123 / 456");
  assert.equal(compactDiscordUrl("https://discord.com/channels/@me/456"), "DM / 456");
});
