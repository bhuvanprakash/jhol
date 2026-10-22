for (let i = 0; i < 3; i++) console.log("i" + i);
for (let a = 0; a < 2; a++) { for (let b = 0; b < 2; b++) console.log(a + "," + b); }
outer: for (let p = 0; p < 3; p++) {
  for (let q = 0; q < 3; q++) {
    if (q === 1) continue outer;
    console.log("L" + p + "-" + q);
  }
}
let c = 0;
loop: for (let r = 0; r < 3; r++) { c++; if (c === 2) break loop; console.log("p" + r); }
