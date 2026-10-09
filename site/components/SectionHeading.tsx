export function SectionHeading({ first, second, center = false }: { first: string; second: string; center?: boolean }) {
  return (
    <h2 className={`heading-lg ${center ? "text-center" : ""}`}>
      {first}
      <br />
      <span className="text-subtle">{second}</span>
    </h2>
  );
}
