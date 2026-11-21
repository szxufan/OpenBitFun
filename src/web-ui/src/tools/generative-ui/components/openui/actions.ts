import { defineAction } from "@openuidev/react-lang";
import { z } from "zod/v4";

export const CopyToClipboard = defineAction({
  name: "CopyToClipboard",
  description: "Copy text to the clipboard, e.g. a shopping list built with @Join",
  params: z.object({ text: z.string() }),
});

export const actions = [CopyToClipboard];
