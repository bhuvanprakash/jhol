const obj = {};
Object.defineProperty(obj, "base", { value: 21, writable: true });
Object.defineProperty(obj, "double", { get: function() { return this.base * 2; } });
console.log(obj.base);
console.log(obj.double);
const literal = {
  _n: 5,
  get n() { return this._n; },
  set n(v) { this._n = v; }
};
console.log(literal.n);
literal.n = 15;
console.log(literal.n);
const full = {
  first: "John",
  last: "Doe",
  get name() { return this.first + " " + this.last; }
};
console.log(full.name);
