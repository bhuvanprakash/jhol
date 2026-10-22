let i = 0;
while (i < 5) { console.log(i); i++; }
let sum = 0;
let n = 0;
while (n < 10) { n++; if (n % 2 === 0) continue; sum += n; }
console.log(sum);
let j = 0;
while (true) { if (j >= 3) break; console.log("j" + j); j++; }
