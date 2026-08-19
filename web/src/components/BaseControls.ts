import { Input as Primitive } from "@base-ui/react/input";
import { createElement } from "react";

export { Button } from "@base-ui/react/button";
export { Form } from "@base-ui/react/form";
export { Input } from "@base-ui/react/input";

export function Textarea(props: React.ComponentProps<typeof Primitive>) {
  return createElement(Primitive, { render: createElement("textarea"), ...props });
}
