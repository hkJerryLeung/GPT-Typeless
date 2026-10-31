import assert from "node:assert/strict";
import test from "node:test";
import { mergeDictionaryTerms, parseDictionaryCsv } from "../src/components/dictionaryCsv.ts";

test("parses UTF-8 BOM, quoted commas, escaped quotes, newlines and explicit headers", () => {
  const parsed = parseDictionaryCsv('\uFEFF\r\n詞彙\r\n"Acme, Inc."\r\n"Say ""hello"""\r\n"two\nlines"\r\n', true);
  assert.deepEqual(parsed, { terms: ["Acme, Inc.", 'Say "hello"', "two\nlines"], emptyRows: 1, headerSkipped: true });
  assert.deepEqual(parseDictionaryCsv("word\nAlice\n").terms, ["word", "Alice"]);
});

test("rejects malformed or ambiguous CSV before any merge", () => {
  for (const input of ['"unfinished', 'bad"quote', '"closed"oops', "one,two", 'valid\n"still,bad",extra', "bad\0value"]) {
    assert.throws(() => parseDictionaryCsv(input), Error, input);
  }
  assert.throws(() => parseDictionaryCsv(" \n\n"), /沒有可匯入/);
  assert.throws(() => parseDictionaryCsv("header\n", true), /標題之後沒有/);
  assert.throws(() => parseDictionaryCsv("ok\nother,extra"), /第 2 行/);
});

test("merges only new terms and preserves existing entries, aliases, and IDs", () => {
  const existing = [
    { id: "old", term: "WWT", aliases: ["double-u"] },
    { id: "accent", term: "Café", aliases: ["cafe"] },
  ];
  const original = structuredClone(existing);
  const ids = ["old", "new-1", "new-2"];
  const parsed = parseDictionaryCsv('wwt\nCAFE\u0301\nNew phrase\nnew PHRASE\n"Acme, Inc."\n');
  const merged = mergeDictionaryTerms(existing, parsed.terms, () => ids.shift());
  assert.equal(merged.added, 2);
  assert.equal(merged.skippedExisting, 2);
  assert.equal(merged.skippedRepeated, 1);
  assert.deepEqual(existing, original);
  assert.strictEqual(merged.entries[0], existing[0]);
  assert.strictEqual(merged.entries[1], existing[1]);
  assert.deepEqual(merged.entries.slice(2), [
    { id: "new-1", term: "New phrase", aliases: [] },
    { id: "new-2", term: "Acme, Inc.", aliases: [] },
  ]);
  const repeated = mergeDictionaryTerms(merged.entries, parsed.terms, () => { throw new Error("should not allocate IDs"); });
  assert.equal(repeated.added, 0);
  assert.equal(repeated.skippedExisting, 5);
  assert.deepEqual(repeated.entries, merged.entries);
});

test("refuses an ID collision without modifying existing entries", () => {
  const existing = [{ id: "same", term: "existing", aliases: ["kept"] }];
  assert.throws(() => mergeDictionaryTerms(existing, ["new"], () => "same"), /識別碼/);
  assert.deepEqual(existing, [{ id: "same", term: "existing", aliases: ["kept"] }]);
});
