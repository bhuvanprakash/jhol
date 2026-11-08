// Array patterns follow the iterator protocol, so any iterable destructures.
// A real array passes through unchanged, so the common path allocates nothing.
const [x, y] = new Set([3, 4]);
console.log(x, y);

const [a, b] = new Map([["k", 1]]);
console.log(JSON.stringify(a), JSON.stringify(b));

function* g() { yield 1; yield 2; }
const [p, q] = g();
console.log(p, q);

const [c, d] = "ab";
console.log(c, d);

const [h, ...t] = new Set([1, 2, 3]);
console.log(h, t.join(","));

const [m, n = 9] = [1];
console.log(m, n);

const [z] = [];
console.log(z);

const arr = [1, 2];
const [u, v] = arr;
console.log(u, v);

function fromParam([e, f]) { return e + f; }
console.log(fromParam(new Set([4, 5])));
