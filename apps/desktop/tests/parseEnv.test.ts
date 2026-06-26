/**
 * Unit tests for parseEnv — the host-settings environment textarea parser.
 *
 * Regression: a value pasted from a shell config with surrounding quotes
 * (`CLAUDE_CONFIG_DIR="/proj/.../claude"`) used to keep the quotes as part of the
 * value, so the launched claude saw a directory literally named with quotes and
 * reported "not logged in".
 */
import { describe, it, expect } from "vitest";
import { parseEnv } from "../src/bridge/api";

describe("parseEnv", () => {
  it("parses a plain KEY=VALUE line", () => {
    expect(parseEnv("FOO=bar")).toEqual([["FOO", "bar"]]);
  });

  it("strips surrounding double quotes from the value", () => {
    expect(parseEnv('CLAUDE_CONFIG_DIR="/proj/x/claude"')).toEqual([
      ["CLAUDE_CONFIG_DIR", "/proj/x/claude"],
    ]);
  });

  it("strips surrounding single quotes from the value", () => {
    expect(parseEnv("K='/a/b c'")).toEqual([["K", "/a/b c"]]);
  });

  it("tolerates a leading `export ` (pasted from .bashrc)", () => {
    expect(parseEnv('export CLAUDE_CONFIG_DIR="/p/c"')).toEqual([
      ["CLAUDE_CONFIG_DIR", "/p/c"],
    ]);
  });

  it("keeps interior `=` in the value and does not strip mismatched quotes", () => {
    expect(parseEnv("URL=https://x/y?a=b")).toEqual([["URL", "https://x/y?a=b"]]);
    expect(parseEnv('Q="unbalanced')).toEqual([["Q", '"unbalanced']]);
  });

  it("ignores blank lines and lines with no '=' or empty key", () => {
    expect(parseEnv("\n  \nFOO=1\n=novalue\nbar")).toEqual([["FOO", "1"]]);
  });

  it("parses multiple lines", () => {
    expect(parseEnv('A=1\nexport B="two"')).toEqual([
      ["A", "1"],
      ["B", "two"],
    ]);
  });
});
