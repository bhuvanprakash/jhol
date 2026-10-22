const arr = [];
for (let i = 0; i < 200000; i++) arr.push(i % 1000);
const sum = arr.map(x => x * 2).filter(x => x % 3 === 0).reduce((a, b) => a + b, 0);
console.log(sum);
