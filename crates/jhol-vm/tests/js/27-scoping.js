console.log(hoisted());
function hoisted() { return "fn hoisted"; }
let x = 5;
{
  let x = 10;
  console.log(x);
}
console.log(x);
const y = 7;
console.log(y);
let z = 1;
z = 2;
console.log(z);
{
  let w = "block";
  console.log(w);
}
