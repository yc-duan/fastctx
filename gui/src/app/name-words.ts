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
      return "请填写名称";
    case "too_long":
      return "名称过长：最多 32 个字符，中文最多 21 个";
    case "space":
      return "名称不能包含空格";
    case "not_allowed":
      return invisible(problem.character) ? "名称包含不可见字符" : `名称不能包含「${problem.character}」，仅限字母、数字和连字符`;
    case "bad_start":
      return problem.character === "-" ? "名称不能以连字符开头" : `名称不能以「${problem.character}」开头`;
    case "ends_with_hyphen":
      return "名称不能以连字符结尾";
    case "mixed_scripts":
      return `「${problem.first}」与「${problem.second}」属于易混淆的文字，不能用于同一名称`;
    case "reserved":
      return "该名称为中枢服务器保留";
  }
};

export const enNameSaid: NameSaid = (problem) => {
  switch (problem.problem) {
    case "empty":
      return "Enter a name";
    case "too_long":
      return "That name is too long: 32 characters at most, 21 in Chinese";
    case "space":
      return "A name cannot contain spaces";
    case "not_allowed":
      return invisible(problem.character) ? "That name contains an invisible character" : `A name cannot contain “${problem.character}”: use letters, digits and hyphens`;
    case "bad_start":
      return problem.character === "-" ? "A name cannot start with a hyphen" : `A name cannot start with “${problem.character}”`;
    case "ends_with_hyphen":
      return "A name cannot end with a hyphen";
    case "mixed_scripts":
      return `“${problem.first}” and “${problem.second}” are from scripts that cannot be mixed in one name`;
    case "reserved":
      return "That name is reserved for the hub";
  }
};
