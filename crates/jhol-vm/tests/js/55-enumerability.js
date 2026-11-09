// Per-key enumerability: `Error.prototype.message` exists but is not
// enumerable, built-in prototypes expose no enumerable keys, and ordinary
// objects are unaffected.
const e = new Error("boom");
console.log("json", JSON.stringify(e));
console.log("keys", JSON.stringify(Object.keys(e)));
const seen = [];
for (const k in e) seen.push(k);
console.log("forin", JSON.stringify(seen));
console.log("pie", e.propertyIsEnumerable("message"), "msg", e.message);
console.log("own", Object.getOwnPropertyNames(e).includes("message"));
console.log("objproto", JSON.stringify(Object.keys(Object.prototype)));
const o = { a: 1, b: 2 };
console.log("plain", JSON.stringify(Object.keys(o)), o.propertyIsEnumerable("a"));
const plainSeen = [];
for (const k in o) plainSeen.push(k);
console.log("forin-plain", plainSeen.join(","));
