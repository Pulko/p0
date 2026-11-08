export interface User {
  name: string;
  id: number;
}

export function formatUser(user: User): string {
  return `${user.id}:${user.name}`;
}
