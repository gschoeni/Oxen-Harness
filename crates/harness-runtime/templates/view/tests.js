window.oxenView.test('The editor is accessible', ({assert}) => {
  const input=document.querySelector('#note');
  assert(input instanceof HTMLTextAreaElement,'Expected a textarea');
  assert(document.querySelector('label[for=note]'),'Expected a visible editor label');
  assert(document.querySelector('#save')?.textContent==='Save','Expected a save action');
});
window.oxenView.test('The fixture is readable JSON',async ({assert}) => {
  const settings=await (await fetch('./settings.json')).json();
  const snapshot=await window.oxenView.read(settings.document);
  assert(typeof JSON.parse(snapshot.content).text==='string','Expected document.text');
});
