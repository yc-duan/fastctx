// The specimen's account skills, each a folder, kept for the page's life
// with the hub's own rules on names and versions; a folder to import, as
// one picked on this device.

import { SKILL_FILE, readSkillText, skillText, textFile, fileText, type PickedFolder, type SkillCalls, type SkillFile, type SkillFolder } from "../world/skills";

const pause = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** A small picture, as a skill's folder may hold one beside its words. */
const PICTURE = "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAYAAADED76LAAAAJ0lEQVR4nGP8z8DwnwEJMDEwMDAyMDAwMv5nYGBiQAP/kRUwMDAAAMIZBQ1g2GV8AAAAAElFTkSuQmCC";

function skill(name: string, description: string, instructions: string, files: SkillFile[] = []): SkillFolder {
  return { name, description, version: 1, files: [textFile(SKILL_FILE, skillText(name, description, instructions)), ...files] };
}

/** A folder as kept: named and described by its SKILL.md. */
function folder(name: string, version: number, files: SkillFile[]): SkillFolder {
  const said = readSkillText(fileText(files.find((file) => file.path === SKILL_FILE) ?? { path: SKILL_FILE, data: null }) ?? "");
  return { name: said.name ?? name, description: said.description ?? "", version, files };
}

export function specimenSkills(): SkillCalls {
  const kept: SkillFolder[] = [
    skill(
      "release-notes",
      "写发版说明、更新日志时：先列用户看得见的变化，再列修好的问题",
      "## 发版说明\n\n1. 用 `scripts/changelog.py` 列出这一版与上一版之间的提交。\n2. 先写用户看得见的变化，一条一句。\n3. 再写修好的问题，写清以前怎样、现在怎样。\n4. 照 `templates/notes.md` 的样子排。不写内部重构。",
      [
        textFile("scripts/changelog.py", 'import subprocess\nimport sys\n\n\ndef commits(since: str) -> list[str]:\n    out = subprocess.check_output(["git", "log", "--format=%s", f"{since}..HEAD"], text=True)\n    return [line for line in out.splitlines() if line]\n\n\nif __name__ == "__main__":\n    for subject in commits(sys.argv[1]):\n        print(f"- {subject}")\n'),
        textFile("templates/notes.md", "# {版本}\n\n## 看得见的变化\n\n- \n\n## 修好的问题\n\n- \n"),
        { path: "templates/header.png", data: PICTURE },
      ],
    ),
    skill(
      "paper-review",
      "审一篇论文或技术报告时用：按贡献、证据、复现三件事给意见",
      "## 审稿\n\n照 `checklist.md` 逐条过：\n\n- 贡献：它声称做成了什么，和已有的比新在哪里。\n- 证据：每个结论由哪张图、哪组实验支撑。\n- 复现：数据、代码、超参数是否给全。",
      [textFile("checklist.md", "- [ ] 贡献说清了吗\n- [ ] 每个结论都有证据\n- [ ] 数据、代码、超参数给全了\n")],
    ),
    skill("lab-safety", "动机房的机器之前：先看有没有人在用，晚上十点后不重启", "先 `w` 看谁在线、跑着什么作业；有人在用就别动。晚上十点之后不重启任何一台。"),
  ];
  const picked: PickedFolder = {
    folder: "Deploy-Checks",
    files: [
      textFile(SKILL_FILE, "---\ndescription: 上线之前：跑一遍冒烟检查，看监控有没有报警\n---\n\n先跑 `run.sh`，全绿再上线；有报警先找值班的人。\n"),
      { ...textFile("run.sh", "#!/bin/sh\nset -e\ncurl -fsS http://localhost:8080/health\n"), executable: true },
    ],
  };
  return {
    list: async () => {
      await pause(120);
      return kept.map(({ name, description }) => ({ name, description }));
    },
    read: async (_hub, _account, name) => {
      await pause(80);
      const one = kept.find((skill) => skill.name === name);
      return one ? structuredClone(one) : null;
    },
    write: async (_hub, _account, name, expected, files) => {
      await pause(300);
      if (expected !== null || kept.some((one) => one.name === name)) return false;
      kept.push(folder(name, 1, files));
      return true;
    },
    edit: async (_hub, _account, name, expected, changes) => {
      await pause(300);
      const at = kept.findIndex((one) => one.name === name);
      if (at < 0 || kept[at]!.version !== expected) return false;
      let files = kept[at]!.files;
      for (const change of changes) {
        files = files.filter((file) => file.path !== (change.change === "put" ? change.file.path : change.path));
        if (change.change === "put") files.push(change.file);
      }
      const made = folder(name, expected + 1, files);
      if (made.name !== name && kept.some((one) => one.name === made.name)) return false;
      kept[at] = made;
      return true;
    },
    remove: async (_hub, _account, name, expected) => {
      await pause(200);
      const at = kept.findIndex((one) => one.name === name);
      if (at < 0 || kept[at]!.version !== expected) return false;
      kept.splice(at, 1);
      return true;
    },
    pick: async () => {
      await pause(200);
      return structuredClone(picked);
    },
  };
}
