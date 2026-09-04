document.querySelector('[data-copy]').addEventListener('click', async (event) => {
  const button = event.currentTarget;
  const text = document.getElementById(button.dataset.copy).textContent;
  try {
    await navigator.clipboard.writeText(text);
    button.textContent = 'Copied';
    document.getElementById('copy-status').textContent = ' Commands copied.';
  } catch {
    const selection = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(document.getElementById(button.dataset.copy));
    selection.removeAllRanges(); selection.addRange(range);
    document.getElementById('copy-status').textContent = ' Select and copy the commands above.';
  }
});
