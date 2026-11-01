// Two bugs found here, both fixed:
// 1. Indexed access switched to a slow generic path past index 4096 (an
//    arbitrary ceiling) — 98% of a 200k-element array paid a full property
//    lookup per access.
// 2. `set` routed indices >= SPARSE_FROM into the sparse map while `get`
//    read `data` first, so writes at 4096 <= i < data.len() were lost.
// 3. Object numeric keys: the fused literal path stored Str("5000") while
//    computed access produced Idx(5000), so `{5000: v}` was unreadable.
let a = [];
for (let i = 0; i < 12000; i++) a.push(i * 2);
console.log(a.length, a[0], a[4095], a[4096], a[11999]);
console.log(a[12000], a[99999]);
a[4096] = -7;
a[11999] = -8;
console.log(a[4096], a[11999]);
a.push(1);
console.log(a.length, a[12000]);
let sum = 0;
for (let i = 0; i < a.length; i++) sum += a[i];
console.log("sum", sum);
// sparse / growth beyond length still goes through the generic path
let b = [1];
b[7] = 9;
console.log(b.length, b[7], b[3]);
const o = { 5000: "x" };
console.log(o[5000], a[5000]);
let p = {};
p[7000] = "w";
console.log(p["7000"], p[7000]);
console.log("done");
