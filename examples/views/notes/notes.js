const api = window.oxenView;
const field = (id) => document.getElementById(id);
let snapshot, loadedPath;
if (api.context.target.path) field("path").value = api.context.target.path;
async function perform(work) {
  field("error").textContent = "";
  try { await work(); } catch (error) { field("error").textContent = String(error); }
}
const content = () => JSON.stringify({ title: field("title").value, body: field("body").value }, null, 2) + "\n";
const adopt = (doc) => {
  const note = JSON.parse(doc.content);
  if (typeof note.title !== "string" || typeof note.body !== "string") throw new Error("A note needs title and body strings.");
  snapshot = doc; loadedPath = doc.path;
  field("title").value = note.title; field("body").value = note.body;
  field("status").textContent = "Saved to project";
};
field("load").onclick = () => perform(async () => {
  adopt(await api.read(field("path").value));
  await api.report({ status: "mounted", dirty: false });
});
field("create").onclick = () => perform(async () => adopt(await api.save(field("path").value, content())));
field("save").onclick = () => perform(async () => {
  if (!snapshot || loadedPath !== field("path").value) throw new Error("Open this file first, or use Create new.");
  const draft = content();
  const saved = await api.save(loadedPath, draft, snapshot.revision);
  snapshot = saved;
  field("status").textContent = content() === draft ? "Saved to project" : "Unsaved changes";
});
for (const id of ["title", "body"]) field(id).oninput = () => {
  field("status").textContent = "Unsaved changes";
  perform(() => api.report({ status: "mounted", dirty: true }));
};
