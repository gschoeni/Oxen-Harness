// Skills as slash commands: typing `/<skill-name> [request]` runs that skill.
//
// The skill's instructions are baked into the prompt at send time, the way
// staged snippets are, instead of asking the model to load the skill through
// its `skill` tool. That tool is built when a chat starts, so a skill written
// a minute ago (or with this chat already open) would be unknown to it; the
// file on disk is always current. The block rides at the end of the message
// in a fixed shape, so the transcript can fold it into a chip.

import type { SkillInfo } from "./types";

/** A skill block as it was baked into a sent message. */
export interface SkillBlock {
  name: string;
  dir: string;
  instructions: string;
}

const BLOCK = /\n\n<skill name="([a-z0-9-]+)" dir="([^"]*)">\n([\s\S]*)\n<\/skill>$/;

const escapeAttr = (s: string) => s.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
const unescapeAttr = (s: string) => s.replace(/&quot;/g, '"').replace(/&amp;/g, "&");

/** Skills that can run as `/<name>`: on, and finished (a draft has nothing
 *  to follow yet). */
export function invocableSkills(skills: SkillInfo[]): SkillInfo[] {
  return skills.filter((s) => s.enabled && !s.draft);
}

/** Separate a sent message from the skill block baked into it, if any. */
export function splitSkillBlock(text: string): { text: string; skill: SkillBlock | null } {
  const match = BLOCK.exec(text);
  if (!match) return { text, skill: null };
  const [, name, dir, instructions] = match;
  return {
    text: text.slice(0, match.index),
    skill: { name, dir: unescapeAttr(dir), instructions },
  };
}

/** `/<skill> request` → the message plus that skill's instructions. Anything
 *  else (another command, plain text, an already-expanded message) comes
 *  back unchanged. */
export function withSkillInstructions(text: string, skills: SkillInfo[]): string {
  const match = /^\/([a-z0-9-]+)(?:\s|$)/.exec(text);
  if (!match || splitSkillBlock(text).skill) return text;
  const skill = invocableSkills(skills).find((s) => s.name === match[1]);
  if (!skill) return text;
  return (
    `${text}\n\n<skill name="${skill.name}" dir="${escapeAttr(skill.dir)}">\n` +
    `The user invoked the "${skill.name}" skill. Follow these instructions for ` +
    `their request; supporting files live in ${skill.dir}.\n\n` +
    `${skill.instructions}\n</skill>`
  );
}
