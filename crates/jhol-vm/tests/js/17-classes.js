class Shape {
  constructor(name) { this.name = name; }
  describe() { return this.name + " shape"; }
}
class Circle extends Shape {
  constructor(r) { super("circle"); this.r = r; }
  area() { return 3 * this.r * this.r; }
  static unit() { return new Circle(1); }
}
const s = new Shape("sq");
console.log(s.describe());
const c = new Circle(2);
console.log(c.describe());
console.log(c.area());
console.log(Circle.unit().area());
class Box {
  constructor() { this._v = 0; }
  get v() { return this._v; }
  set val(x) { this._v = x; }
}
const b = new Box();
b.val = 42;
console.log(b.v);
