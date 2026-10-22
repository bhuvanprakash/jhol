// Arguments to a call must land in adjacent registers. Evaluating an
// argument used to drag the temp cursor back, so later arguments were
// written where the call never read them.
let o = { a: 1, b: 2, c: 3 };
console.log(o.a, o.b);
console.log("x", o.a, o.b);
console.log(o.a, o.b, o.c);
function f(x, y, z) { return x * 100 + y * 10 + z; }
console.log(f(o.a, o.b, o.c));
console.log(f(o.a + 1, o.b * 2, o.c - 1));
console.log(o.a + o.b);
let s = { n: 7 };
console.log(s.n, s.n);
console.log("head", s.n, s.n, s.n);
class K { m(x, y) { return x + y; } }
let k = new K();
console.log(k.m(o.a, o.b));
console.log(k.m(o.a, o.b), k.m(o.b, o.c));
console.log([o.a, o.b].length, [o.a, o.b, o.c].length);
let arr = [10, 20];
console.log(arr[0], arr[1], arr[0] + arr[1]);
console.log("done");
// Array literals, `new` and constructor calls share the same contiguous-window
// requirement as ordinary calls.
let oa = { a: 4, b: 5, c: 6 };
console.log([oa.a, oa.b, oa.c].join(","));
console.log(JSON.stringify([oa.a, oa.b]));
function C(x, y, z) { this.x = x; this.y = y; this.z = z; }
let cc = new C(oa.a, oa.b, oa.c);
console.log(cc.x, cc.y, cc.z);
class D { constructor(p, q) { this.p = p; this.q = q; } }
let dd = new D(oa.a, oa.b);
console.log(dd.p, dd.q);
console.log(String(oa.a) + String(oa.b));
let deep = { m: { n: 9 } };
console.log(deep.m.n, deep.m.n, deep.m.n + 1);
