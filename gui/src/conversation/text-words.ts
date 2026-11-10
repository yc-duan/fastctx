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
  attached: "文字太长，已转成文本附件",
  copied: "文字太长，未能添加附件；全文已复制到剪贴板",
  limit: "输入框已写满，请将更多文字作为文件添加",
  saving: "文字太长，正在转成文本附件…",
  rescue: "未能添加附件或自动复制，请复制并保留这份全文",
  copy: "复制全文并继续",
  close: "关闭",
  reconciled: "草稿在别处改过，已保留你的修改",
  changedSaving: "草稿在别处改过，正在保留未提交的文字…",
  changedCopied: "草稿在别处改过，未提交的全文已复制到剪贴板",
  changedRescue: "请复制并保留未提交的全文，复制后即可继续编辑",
  changedText: "未提交的文字",
};

export const enText: TextWords = {
  file: "Pasted text.txt",
  attached: "The text was too long and is now a text attachment",
  copied: "The text was too long to attach; the full text was copied to the clipboard",
  limit: "This input is full; add more text as a file",
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
