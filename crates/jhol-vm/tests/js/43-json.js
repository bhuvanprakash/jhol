// JSON.parse/stringify semantics: strict tokens, number grammar, escapes,
// surrogate pairs, round-trips.
const o = JSON.parse('{"a": 1, "b": [true, false, null], "c": {"d": "x"}}');
console.log(o.a, o.b[0], o.b[2], o.c.d);
console.log(JSON.parse('"\\u0041\\u00e9\\u4e2d"'));
console.log(JSON.parse('"\\ud83d\\ude00"'));
console.log(JSON.parse('"\\ud83d"') === "\uFFFD");
console.log(JSON.parse("1e21"), JSON.parse("-0"), JSON.parse("0.5"), JSON.parse("1e-7"));
console.log(JSON.parse("  [ 1 , 2 ]  "));
console.log(JSON.parse('"\\n\\t\\r\\b\\f\\/\\\\\\\\"'));
console.log(JSON.parse("true") && JSON.parse("false") === false && JSON.parse("null") === null);
const round = JSON.parse(JSON.stringify({ s: "a\"b\\c\nd", n: 1.5, big: 1e21, arr: [1, [2]], e: {} }));
console.log(round.s.length, round.n, round.big, round.arr[1][0], Object.keys(round.e).length);
function throws(f) { try { f(); return false; } catch (e) { return true; } }
console.log(throws(() => JSON.parse("truX")));
console.log(throws(() => JSON.parse("[1,]")));
console.log(throws(() => JSON.parse("{a:1}")));
console.log(throws(() => JSON.parse("01")));
console.log(throws(() => JSON.parse('"\\uZZZZ"')));
console.log(throws(() => JSON.parse("1. ")));
console.log(throws(() => JSON.parse("nope")));
console.log(throws(() => JSON.parse("[1] extra")));
console.log(JSON.stringify(undefined), JSON.stringify(1/0), JSON.stringify(-1/0), JSON.stringify(NaN));
console.log(JSON.stringify({ u: undefined, f: () => 1, n: null }), JSON.stringify([undefined, 2]));
