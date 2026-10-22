const objs = [];
for (let i = 0; i < 200000; i++) objs.push({ a: i % 10, b: i % 7, c: i % 3, d: i % 5 });
let sum = 0;
for (let i = 0; i < objs.length; i++) { const o = objs[i]; sum += o.a * 1000 + o.b * 10 + o.c + o.d; }
console.log(sum);
