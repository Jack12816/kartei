// A sample TSX component module for the extraction tests.

interface GreetingProps {
  name: string;
}

export const Greeting = ({ name }: GreetingProps) => (
  <p>Hello {name}</p>
);

export default function App(): JSX.Element {
  return <Greeting name="world" />;
}
