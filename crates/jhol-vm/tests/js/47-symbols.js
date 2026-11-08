// Symbols are primitives here: a tagged interned id, not a string.
// `typeof` says "symbol", two calls are distinct, `for`/`keyFor` share.
console.log(typeof Symbol("x"));
console.log(Symbol("x") === Symbol("x"), Symbol.for("k") === Symbol.for("k"));
console.log(String(Symbol("d")));
console.log(Symbol.keyFor(Symbol.for("k")), Symbol.keyFor(Symbol("n")));

// Identity is the whole point of a symbol key: two symbols with the same
// description must not collide.
const a = Symbol("key");
const b = Symbol("key");
const o = {};
o[a] = 1;
console.log(o[a], o[b]);

// Implicit conversion throws; the explicit String() does not.
try { let s = `${Symbol("t")}`; console.log("no throw", s); }
catch (e) { console.log("throws"); }
