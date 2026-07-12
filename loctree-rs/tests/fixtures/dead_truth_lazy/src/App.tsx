// example-app pattern: React.lazy() extracting a NAMED export from a dynamic import.
const InviteTeam = () =>
  import('./Steps').then((m) => ({ default: m.InviteTeamStep }));

export function App() {
  return InviteTeam;
}
