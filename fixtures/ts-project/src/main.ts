import { User, formatUser } from "./user";

function greet(user: User) {
  return "Hello " + user.name;
}

const alice: User = { name: "Alice", id: 1 };
greet(alice);
formatUser(alice);

export const broken: User = { name: "x" };
