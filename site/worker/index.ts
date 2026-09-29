import { CONTACT_PATH } from "../src/contact";
import { missing } from "./assets";
import { contact } from "./contact";

export default {
  fetch(request, env) {
    return new URL(request.url).pathname === CONTACT_PATH
      ? contact(request, env)
      : missing(request, env);
  },
} satisfies ExportedHandler<Env>;
