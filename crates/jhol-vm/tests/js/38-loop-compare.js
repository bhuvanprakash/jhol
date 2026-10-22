// fused-compare edge cases: must match node exactly
let a = 0; for (let i = 0; i < 5; i++) a += i; console.log(a);
let b = 10; while (b >= 7) b--; console.log(b);
let c = 0; for (let i = 10; i > 0; i--) c += i; console.log(c);
let d = 0; for (let i = 0; i <= 3; i++) { if (i >= 1) d += i; } console.log(d);
let n = 0; for (let i = 0; i !== 4; i++) n++; console.log(n);
let f = 0; for (let i = 0.5; i < 2; i += 0.5) f++; console.log(f);
let nan = 0; for (let i = 0; i <= NaN; i++) nan++; console.log(nan);
let s = 0; let k = "5"; for (let i = 0; i < 3; i++) { if (k > "3") s += 1; } console.log(s);
let rev = 0; for (let i = 0; i < 3; i++) { if (5 > i) rev++; } console.log(rev);
let w = 0; let j = 0; while (j < 3 && j !== 1) { w += j; j++; } console.log(w);
let cap = 0; const lim = 4; for (let i = 0; i < lim; i++) cap += i; console.log(cap);
let obj = { v: 0 }; for (let i = 0; i < 3; i++) obj.v += i; console.log(obj.v);
let dec = 0; for (let i = 3; i-- > 0;) dec += i; console.log(dec);
let empty = 0; for (;;) { empty++; if (empty > 2) break; } console.log(empty);
console.log("done");
