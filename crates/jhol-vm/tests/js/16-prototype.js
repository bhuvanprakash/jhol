function Animal(name) { this.name = name; }
Animal.prototype.speak = function() { return this.name + " makes a sound"; };
const a = new Animal("Rex");
console.log(a.name);
console.log(a.speak());
console.log(a instanceof Animal);
console.log(Object.getPrototypeOf(a) === Animal.prototype);
function Dog(name) { Animal.call(this, name); }
Dog.prototype = Object.create(Animal.prototype);
Dog.prototype.bark = function() { return this.name + " barks"; };
const d = new Dog("Fido");
console.log(d.bark());
console.log(d instanceof Dog);
console.log(d instanceof Animal);
console.log(Object.getPrototypeOf(Dog.prototype) === Animal.prototype);
