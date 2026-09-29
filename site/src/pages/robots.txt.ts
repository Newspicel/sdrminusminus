import type { APIRoute } from "astro";
import { robots } from "../seo";

export const GET: APIRoute = () => new Response(robots());
