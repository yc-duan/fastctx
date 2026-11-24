// Model collaboration as the specimen's account has it: its Claude logins
// may ask the reviewer, a GPT on this computer; the scout waits, turned off,
// as the World offers it. The texts are the World's own.

import type { CollabView } from "../world/collab";

const REVIEW = "A colleague is GPT, run by Codex. It takes a piece of work you hand it whole and does it on its\nown: it reads all of the material before it judges, stays with a hard question for hours, tries one\nway after another, and says plainly what it found and how sure it is.\n\nA colleague is not a subagent. If your host gives you subagents of its own, they work inside your\nturn, often on a smaller model: use them for quick, wide, cheap work whose result you fold back at\nonce, such as finding where something is, reading many places, or pulling facts out. A colleague is\na model at full strength, apart from you, and another maker's unless you are GPT too. It works for\nas long as the work takes, outside your turn and after it. Use it for a judgment that must not lean\non yours, for work that takes hours, for experiments on the World's machines, and for anything that\nshould go on while you do something else or end your turn. If your host gives you no subagents, a\ncolleague is also how you hand off a piece of work to run apart.\n\nHand it work that ends in an answer, or in something it makes apart from what you keep:\n- review: a diff or a branch, to find every way it fails; an idea, a design or a plan before you\n  act on it, to find where it does not hold;\n- look into: a question of fact, logs or data to read, sources to find and weigh, a counterexample\n  to construct, a number to measure;\n- try out: the ways you name, each tried in a small experiment or prototype in a scratch place of\n  its own, to bring back what each showed and what it cost;\n- make: a draft, a report, a summary, a translation, an analysis, a document or a specification,\n  a text people or models will read, handed back for you to take or leave.\n\nDo not hand it the design, nor a share of building what you keep: code that goes into your\nrepositories, changes to your working trees or documents. A colleague handed such a branch came back\non a base that had moved on and held to the letter of what it was asked; that work you design,\nbuild, join and verify yourself, and a colleague changes nothing you will keep.\n\nSay what you need and why, what you already know and have decided, where the material is (the\nmachines and paths), and what to bring back. From a review: each problem with how serious it is,\nwhere, under what inputs or order of events, what follows, and whether it checked it or infers it.\nFrom other work: what it found or made, where it is, how it knows and how sure it is. From any: what\nit could not do or check. Keys, credentials and work that may trip its maker's safety checks stay\nwith you.\n\nWhat comes back is material for your judgment, not a verdict. What it found, checked and measured is\nusually sound; what it suggests, a fix, a better way, how something should be built, is design, and\nit settles easily on a local optimum: doubt every suggestion wholly and make the design yourself.\nPoint it at exactly what to look into, check or try, and take its facts. Take from it only what\nholds as fact, has a clear consequence, and leads the whole work toward its best: a true finding\nwhose remedy would add machinery for a rare corner, or better one part at the cost of the whole, may\nbe left as it is, and said so. Check what matters, decide what to take, and do the rest yourself.\nWork that matters can go to several colleagues at once, each from its own angle, or to one that has\nnot seen your conclusion, so that it is not anchored on it. Keep each colleague's id and what it\ndoes in your plan.\n\nWhile it works it messages you only when it cannot sensibly go on without you, and that wakes you.\nReply when it asks you something; send it nothing to acknowledge, nor your passing thoughts, and do\nnot ask it to report as it goes: what it did comes back with the result. Tell it that what it could\nnot do is an honest answer, so a gap never comes back dressed as a finding.";

const SCOUT = "A scout is GPT, run by Codex, sent to look: it finds where something is, reads many places, follows\na trail across files, logs and machines, and brings back what it saw, pressed small and with its\nsources. It reads only. It changes nothing, and it makes no design and settles no choice: those\nstay with you.\n\nSend a scout for reading that would fill your own context: where a thing is defined and used, what\nthere is of a kind, what a set of files or a log says, whether a fact holds, how a name or a\nconvention runs through the code. If your host gives you subagents of its own, they do this inside\nyour turn; a scout does it on any machine it may use, apart from you, and goes on while you do\nsomething else.\n\nGive each scout one clear question that stands on its own: what to find, where to look (the\nmachines and paths), what you already know or have ruled out, and what to bring back. Questions\nthat do not wait on each other go to several scouts at once. A question that needs a judgment, a\ndesign or a change is not a scout's.\n\nWhat comes back is data for you to act on, not a verdict: what it saw, each with where it saw it\n(the machine, the path and line, the exact words where they carry weight), what it infers kept\napart from what it saw, and what it did not cover. Check by its sources whatever your next step\nrests on; what it did not find is not shown to be absent. The judgment, the design and every change\nare yours.";

/** `?collab=off` shows it before it is turned on, with no preset kept;
 *  `?collab=old-hub` as a hub from before presets keeps it. */
export function specimenCollab(): CollabView {
  const scene = new URLSearchParams(location.search).get("collab");
  const view = whole();
  const { shelved: _, ...kept } = view.collab;
  if (scene === "off") return { ...view, collab: { ...kept, version: 0, on: false, callers: [], callees: [] } };
  if (scene === "old-hub") {
    const callees = kept.callees.map(({ preset: _, ...callee }) => callee);
    return { collab: { ...kept, callees }, default_prompt: view.default_prompt };
  }
  return view;
}

function whole(): CollabView {
  return {
    collab: {
      version: 3,
      on: true,
      callers: [
        { machine: "desktop", agent: "claude", home: null },
        { machine: "laptop", agent: "claude", home: null },
        { machine: "server", agent: "claude", home: null },
        { machine: "gpu", agent: "claude", home: null },
      ],
      callees: [
        {
          name: "gpt",
          use_when: "",
          login: { machine: "desktop", agent: "codex", home: null },
          model: "gpt-6.1-sol",
          effort: "high",
          screen: true,
          preset: { kind: "review" },
        },
      ],
      shelved: [],
      prompt: null,
      rules: "",
    },
    default_prompt: REVIEW,
    presets: [
      { kind: "review", does: "It reviews code, ideas and designs before you act on them; looks into questions of fact, data and sources; tries the ways you name in experiments of its own; and makes drafts, reports and other work apart from what you keep. It neither designs nor builds what you keep.", prompt: REVIEW },
      { kind: "scout", does: "It scouts: finds where things are, reads many places and checks facts across files, logs and machines, reading only, and hands back what it saw, pressed small, with its sources. It changes nothing and decides nothing.", prompt: SCOUT },
    ],
    keeps_presets: true,
  };
}
