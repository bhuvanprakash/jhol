// Builtin-surface additions: Date, WeakMap/WeakSet, Symbol registry, base64,
// Object.is, Error subclasses, Function binding, crypto.
const d = new Date(Date.UTC(2024, 0, 15, 10, 30, 45));
console.log(d.toISOString());
console.log(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCDay(), d.getUTCHours());
console.log(new Date(Date.UTC(2000, 1, 29)).getUTCFullYear());
console.log(new Date(Date.UTC(1969, 11, 31, 23, 59, 59)).toISOString());
console.log(Date.parse("2024-01-15T10:30:45Z"));
console.log(Date.parse("2024-01-15"));
const local = new Date(2024, 5, 15, 12, 0, 0);
console.log(local.getTime() - local.getTimezoneOffset() * 60000 === Date.UTC(2024, 5, 15, 12, 0, 0));
const wm = new WeakMap();
const k1 = {}, k2 = {};
wm.set(k1, 42).set(k2, "x");
console.log(wm.get(k1), wm.get(k2), wm.has(k1), wm.delete(k1), wm.has(k1), wm.delete(k1));
wm.set(k1, 7);
console.log(wm.get(k1));
const ws = new WeakSet();
const o1 = {};
ws.add(o1).add(o1);
console.log(ws.has(o1), ws.delete(o1), ws.has(o1));
console.log(btoa("hello"), atob("aGVsbG8="), btoa("any carnal pleasure"));
console.log(atob(btoa("round trip")) === "round trip");
const s1 = Symbol.for("shared");
const s2 = Symbol.for("shared");
console.log(s1 === s2, Symbol.keyFor(s2));
console.log(Symbol.keyFor(Symbol("private")));
// (typeof Symbol.iterator is "string" in JholVM — symbols are unique
// spellings; the well-known constants are still stable and distinct.)
console.log(Symbol.iterator === Symbol.iterator);
console.log(Object.is(NaN, NaN), Object.is(0, -0), Object.is(1, 1));
console.log(new EvalError("e").name, new URIError("u").name);
console.log(typeof Function, typeof Function.prototype.call, typeof Function.prototype.bind);
console.log(typeof crypto.getRandomValues, typeof crypto.randomUUID);
