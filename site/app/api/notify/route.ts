// Saves an address from the email field as a Resend contact in the launch segment. It is used for one email
// when the first version ships (the promise in `notifyNote`), so nothing else is stored with it.

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export async function POST(request: Request) {
  const body = await request.json().catch(() => null);
  // `website` is a field people never see. Bots fill it in, so they get a success and nothing is saved.
  if (body?.website) return new Response(null, { status: 204 });
  const email = typeof body?.email === "string" ? body.email.trim() : "";
  if (email.length > 254 || !EMAIL.test(email)) return Response.json({ error: "INVALID_EMAIL" }, { status: 400 });

  const key = process.env.RESEND_API_KEY;
  const segment = process.env.RESEND_SEGMENT_ID;
  if (!key || !segment) {
    console.error("notify: RESEND_API_KEY or RESEND_SEGMENT_ID is not set");
    return Response.json({ error: "NOT_CONFIGURED" }, { status: 503 });
  }

  const res = await fetch("https://api.resend.com/contacts", {
    method: "POST",
    headers: { Authorization: `Bearer ${key}`, "Content-Type": "application/json" },
    body: JSON.stringify({ email, segments: [{ id: segment }] }),
  });
  if (!res.ok) {
    // The name of Resend's error is enough to find the cause; the message can repeat the address.
    const { name } = await res.json().catch(() => ({ name: "unreadable" }));
    console.error(`notify: Resend answered ${res.status} ${name}`);
    return Response.json({ error: "UPSTREAM" }, { status: 502 });
  }
  return new Response(null, { status: 204 });
}
