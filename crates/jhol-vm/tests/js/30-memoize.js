function memoize(fn) {
  const cache = {};
  return function(n) {
    if (n in cache) return cache[n];
    const result = fn(n);
    cache[n] = result;
    return result;
  };
}
const fib = memoize(function fib(n) {
  return n < 2 ? n : fib(n - 1) + fib(n - 2);
});
console.log(fib(10));
console.log(fib(15));
console.log(fib(20));
