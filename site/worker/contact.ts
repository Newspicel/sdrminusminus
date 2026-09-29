import {
  CONTACT_PAGE,
  type ContactMessage,
  type ContactOutcome,
  parseContact,
} from "../src/contact";

type Mailbox = Pick<Env, "CONTACT_TO" | "CONTACT_FROM">;

const MAX_BODY = 64 * 1024;

const STATUS: Record<ContactOutcome, number> = {
  sent: 200,
  invalid: 400,
  busy: 429,
  failed: 502,
};

export function contactEmail(message: ContactMessage, mailbox: Mailbox): EmailMessageBuilder {
  return {
    to: mailbox.CONTACT_TO,
    from: { email: mailbox.CONTACT_FROM, name: "SDR-- contact form" },
    replyTo: { email: message.email, name: message.name },
    subject: `SDR--: ${message.name}`,
    text: `${message.message}\n\n${message.name} <${message.email}>`,
  };
}

function crossOrigin(request: Request): boolean {
  const origin = request.headers.get("origin");
  return origin !== null && origin !== new URL(request.url).origin;
}

async function deliver(message: ContactMessage, env: Env): Promise<ContactOutcome> {
  try {
    await env.EMAIL.send(contactEmail(message, env));
    return "sent";
  } catch (error) {
    console.error("contact form: sending failed", error);
    return "failed";
  }
}

async function outcomeOf(request: Request, env: Env): Promise<ContactOutcome> {
  const client = request.headers.get("cf-connecting-ip") ?? "unknown";
  const { success } = await env.CONTACT_LIMIT.limit({ key: client });
  if (!success) {
    return "busy";
  }
  if (Number(request.headers.get("content-length") ?? 0) > MAX_BODY) {
    return "invalid";
  }
  const form = await request.formData().catch(() => null);
  if (form === null) {
    return "invalid";
  }
  const parsed = parseContact((name) => form.get(name));
  switch (parsed.kind) {
    case "invalid":
      return "invalid";
    case "trapped":
      console.warn("contact form: trap field filled, message dropped");
      return "sent";
    case "message":
      return deliver(parsed.message, env);
  }
}

function reply(request: Request, outcome: ContactOutcome): Response {
  if (request.headers.get("accept")?.includes("application/json")) {
    return Response.json({ outcome }, { status: STATUS[outcome] });
  }
  return new Response(null, {
    status: 303,
    headers: { Location: `${CONTACT_PAGE}#${outcome}` },
  });
}

export async function contact(request: Request, env: Env): Promise<Response> {
  if (request.method !== "POST") {
    return new Response(null, { status: 405, headers: { Allow: "POST" } });
  }
  if (crossOrigin(request)) {
    return new Response(null, { status: 403 });
  }
  return reply(request, await outcomeOf(request, env));
}
