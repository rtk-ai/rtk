export class Laya {
  static async load() { return new Laya(); }
  async systemOne(state, questions) {
    const answers = {};
    for (const name of Object.keys(questions || {})) answers[name] = { type: "choice", choice: /test/.test(state) ? "test" : "write", confidence: 0.91 };
    return { model: "fake-laya", answers, usage: { chars: String(state || "").length } };
  }
}
