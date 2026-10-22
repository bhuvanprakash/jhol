let x = 5;
if (x < 0) { console.log("neg"); } else if (x === 0) { console.log("zero"); } else { console.log("pos"); }
function grade(s) {
  switch (s) {
    case "A":
      console.log("top");
    case "B":
      console.log("good");
    case "C":
      console.log("pass");
    default:
      console.log("done");
  }
}
grade("A");
let y = 2;
switch (y) {
  case 1:
    console.log("one");
    break;
  case 2:
    console.log("two");
    break;
  default:
    console.log("other");
}
