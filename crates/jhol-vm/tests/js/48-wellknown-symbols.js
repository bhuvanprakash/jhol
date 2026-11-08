// Well-known symbols are real symbols whose id is the internal key the
// protocols use (`@@iterator`), so `obj[Symbol.iterator]` and for-of agree.
// use (`@@iterator`), so `obj[Symbol.iterator]` and `for…of` agree.
console.log(typeof Symbol.iterator, String(Symbol.iterator));
console.log(typeof [][Symbol.iterator]);
console.log([...new Set([1, 2, 2])].join(","));
console.log([..."ab"].join("-"));
function* g() { yield 1; yield 2; }
console.log([...g()].join(","));
console.log(Array.from(new Map([["a", 1]])).join("|"));
let s = "";
for (const c of "ab") { s += c; }
console.log(s);
