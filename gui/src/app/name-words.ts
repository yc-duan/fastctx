// What the person reads when a machine or server name is not one the World
// takes: one plain sentence for each thing the shell's rule finds wrong
// (`fastctx_types::NameProblem`, `world-network.md` 机器的名字).

import type { NameProblem } from "../world/bridge";

export type NameSaid = (problem: NameProblem) => string;

/** A character with nothing to show between quotes. */
const invisible = (character: string): boolean => /^[\p{C}\p{Z}\p{Default_Ignorable_Code_Point}]$/u.test(character);

export const zhNameSaid: NameSaid = (problem) => {
  switch (problem.problem) {
    case "empty":
      return "请填写名字";
    case "too_long":
      return "名字太长：最多 32 个字，中文最多 21 个";
    case "space":
      return "名字里不能有空格";
    case "not_allowed":
      return invisible(problem.character) ? "名字里有看不见的字符" : `名字里不能有「${problem.character}」，只能用字母、数字和连字符`;
    case "bad_start":
      return problem.character === "-" ? "名字不能以连字符开头" : `名字不能以「${problem.character}」开头`;
    case "ends_with_hyphen":
      return "名字不能以连字符结尾";
    case "mixed_scripts":
      return `「${problem.first}」和「${problem.second}」是两种容易看混的文字，不能放在一个名字里`;
    case "reserved":
      return "这个名字留给中枢服务器自己用";
  }
};

export const enNameSaid: NameSaid = (problem) => {
  switch (problem.problem) {
    case "empty":
      return "Enter a name";
    case "too_long":
      return "That name is too long: 32 characters at most, 21 in Chinese";
    case "space":
      return "A name can't contain spaces";
    case "not_allowed":
      return invisible(problem.character) ? "That name contains an invisible character" : `A name can't contain “${problem.character}”: use letters, digits and hyphens`;
    case "bad_start":
      return problem.character === "-" ? "A name can't start with a hyphen" : `A name can't start with “${problem.character}”`;
    case "ends_with_hyphen":
      return "A name can't end with a hyphen";
    case "mixed_scripts":
      return `“${problem.first}” and “${problem.second}” are from scripts that can't be mixed in one name`;
    case "reserved":
      return "That name is reserved for the hub";
  }
};
