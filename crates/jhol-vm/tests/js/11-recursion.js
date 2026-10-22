function fact(n) { return n <= 1 ? 1 : n * fact(n - 1); }
console.log(fact(5));
function gcd(a, b) { return b === 0 ? a : gcd(b, a % b); }
console.log(gcd(48, 36));
function isEven(n) { return n === 0 ? true : isOdd(n - 1); }
function isOdd(n) { return n === 0 ? false : isEven(n - 1); }
console.log(isEven(10));
console.log(isOdd(7));
