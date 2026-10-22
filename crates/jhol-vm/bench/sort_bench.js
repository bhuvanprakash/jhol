let seed = 987654321;
const arr = [];
for (let i = 0; i < 100000; i++) { seed = (seed * 48271) % 2147483647; arr.push(seed); }
arr.sort(function (a, b) { return a - b; });
let sum = 0;
for (let i = 0; i < arr.length; i++) sum += arr[i];
console.log(arr[0] + " " + arr[1] + " " + arr[2] + " " + arr[3] + " " + arr[4] + " " + sum);
