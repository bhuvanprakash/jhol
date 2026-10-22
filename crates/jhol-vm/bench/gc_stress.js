let counter = 0;
for (let i = 0; i < 2000000; i++) { const o = { a: i, b: i + 1, c: i + 2, d: i + 3 }; counter += o.a + o.b + o.c + o.d; }
console.log(counter);
