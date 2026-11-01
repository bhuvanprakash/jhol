function fib(n) {
    if (n < 2) return n;
    return fib(n - 1) + fib(n - 2);
}

var result = fib(35);
if (result !== 9227465) {
    throw new Error("unexpected result: " + result);
}
