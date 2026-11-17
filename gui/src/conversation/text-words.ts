import { MAX_DOCUMENT_CHARACTERS } from "./composer/limit";

export interface TextWords {
  file: string;
  attached: string;
  copied: string;
  limit: string;
  saving: string;
  rescue: string;
  copy: string;
  close: string;
  reconciled: string;
  changedSaving: string;
  changedCopied: string;
  changedRescue: string;
  changedText: string;
}

export const zhText: TextWords = {
  file: "粘贴的文本.txt",
  attached: "文字过长，已转为文本附件",
  copied: "文字过长，无法添加附件；全文已复制到剪贴板",
  limit: `输入框已满（最多 ${MAX_DOCUMENT_CHARACTERS.toLocaleString("en-US")} 字），更多文字请作为文件添加`,
  saving: "文字过长，正在转为文本附件…",
  rescue: "无法添加附件或自动复制，请手动复制全文",
  copy: "复制全文并继续",
  close: "关闭",
  reconciled: "草稿已在别处修改，已保留本处修改",
  changedSaving: "草稿已在别处修改，正在保留未提交的文字…",
  changedCopied: "草稿已在别处修改，未提交的全文已复制到剪贴板",
  changedRescue: "请复制未提交的全文，复制后可继续编辑",
  changedText: "未提交的文字",
};

export const enText: TextWords = {
  file: "Pasted text.txt",
  attached: "The text was too long and is now a text attachment",
  copied: "The text was too long to attach; the full text was copied to the clipboard",
  limit: `This input holds at most ${MAX_DOCUMENT_CHARACTERS.toLocaleString("en-US")} characters; add more text as a file`,
  saving: "The text is too long; creating a text attachment…",
  rescue: "Could not attach or copy the text; copy and keep the full text below",
  copy: "Copy full text and continue",
  close: "Close",
  reconciled: "The draft changed elsewhere; your edits were kept",
  changedSaving: "The draft changed elsewhere; preserving your unsent text…",
  changedCopied: "The draft changed elsewhere; your full unsent text was copied to the clipboard",
  changedRescue: "Copy and keep the full unsent text to continue editing",
  changedText: "Unsent text",
};
