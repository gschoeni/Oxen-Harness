/// <reference path="./oxen-view.d.ts" />
const api = window.oxenView;
const settings = await (await fetch('./settings.json')).json();
const path = api.context.target.path ?? settings.document;
const note = document.querySelector('#note');
const status = document.querySelector('#status');
let snapshot, saving = false;
document.querySelector('#title').textContent = settings.title;
document.title = settings.title;
function message(text, error = false) { status.textContent = text; status.dataset.error = String(error); }
async function retain() {
  await api.report({dirty:true});
  await api.retain({path,text:note.value,revision:snapshot?.revision});
  await api.report({dirty:false,document:path});
}
async function loadSaved() {
  snapshot = await api.read(path);
  note.value = JSON.parse(snapshot.content).text ?? '';
  await api.retain(null);
  await api.report({dirty:false,document:path});
  message('Saved to your workspace');
}
try {
  const draft = await api.restore();
  snapshot = await api.read(path);
  if (draft?.path === path) {
    note.value = draft.text;
    if (draft.revision !== snapshot.revision) {
      snapshot = {...snapshot,revision:draft.revision};
      message('Your draft is restored, but the file changed. Add it to chat to merge, or load the saved version.',true);
    } else message('Your draft was restored');
  } else {note.value=JSON.parse(snapshot.content).text ?? '';message('Saved to your workspace');}
} catch (error) { message(String(error),true); }
note.addEventListener('input',()=>retain().then(()=>message('Draft retained · Save to update the project file')).catch(error=>message(String(error),true)));
document.querySelector('#save').addEventListener('click',async () => {
  if(saving || !snapshot) return;
  saving=true;
  const text=note.value;
  try {
    snapshot=await api.save(path,JSON.stringify({text},null,2)+'\n',snapshot.revision);
    if(note.value===text) {await api.retain(null);await api.report({dirty:false});message('Saved to your workspace');}
    else await retain();
  } catch(error) {message(`${error}. Your draft is retained; add it to chat to merge.`,true);}
  finally {saving=false;}
});
document.querySelector('#reload').addEventListener('click',()=>loadSaved().catch(error=>message(String(error),true)));
document.querySelector('#agent').addEventListener('click',()=>api.addToChat(`Help me improve ${settings.title}. Source: ${settings.source}. Read its AGENTS.md, edit the files, then use develop_view to check and test the preview.\nDocument: ${path}\nCurrent draft:\n${note.value}`).catch(error=>message(String(error),true)));
