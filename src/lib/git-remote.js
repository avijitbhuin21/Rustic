// Turn a git remote URL into the repository's web page, for "Visit remote".
// Handles https / ssh / scp-style remotes for GitHub, GitLab, Bitbucket,
// Azure DevOps (incl. legacy visualstudio.com), AWS CodeCommit, Gitea /
// Forgejo / self-hosted GitLab and any other host that serves the repo at
// https://<host>/<path>.

/** Known hosts → display name + branch-URL style. */
const PROVIDERS = [
  { test: (h) => h === 'github.com' || h.endsWith('.github.com') || h.startsWith('github.'), name: 'GitHub', branch: (b) => `/tree/${b}` },
  { test: (h) => h === 'gitlab.com' || h.startsWith('gitlab.'), name: 'GitLab', branch: (b) => `/-/tree/${b}` },
  { test: (h) => h === 'bitbucket.org' || h.startsWith('bitbucket.'), name: 'Bitbucket', branch: (b) => `/src/${b}` },
  { test: (h) => h === 'dev.azure.com' || h.endsWith('.visualstudio.com'), name: 'Azure DevOps', branch: (b) => `?version=GB${b}` },
  { test: (h) => h === 'codeberg.org', name: 'Codeberg', branch: (b) => `/src/branch/${b}` },
  { test: (h) => h.startsWith('gitea.') || h.startsWith('forgejo.'), name: 'Gitea', branch: (b) => `/src/branch/${b}` },
];

/** `{ host, path }` of a remote URL (any of the common git URL shapes), or null. */
function parseRemote(raw) {
  const url = String(raw || '').trim();
  if (!url) return null;
  // Local repositories (C:\..., C:/..., /srv/..., file://) have no web page.
  if (/^[a-zA-Z]:[\\/]/.test(url) || url.startsWith('/') || url.startsWith('\\') || url.startsWith('file:')) return null;
  // scp-like: git@host:org/repo.git
  const scp = url.match(/^(?:[^@/\s]+@)?([^:/\s]+):(?!\/\/)(.+)$/);
  if (scp && !/^[a-z][a-z0-9+.-]*:\/\//i.test(url)) return { host: scp[1].toLowerCase(), path: scp[2] };
  try {
    const u = new URL(url);
    if (!/^(https?|ssh|git|git\+ssh):$/.test(u.protocol)) return null;
    return { host: u.hostname.toLowerCase(), path: u.pathname.replace(/^\/+/, '') };
  } catch {
    return null;
  }
}

/**
 * Web page for remote `raw` (optionally at `branch`): `{ url, provider }`, or
 * null when the remote isn't browsable (local path, unknown scheme).
 */
export function remoteWebInfo(raw, branch) {
  const s = String(raw || '').trim();

  // AWS CodeCommit: https://git-codecommit.<region>.amazonaws.com/v1/repos/<repo>
  // or codecommit::<region>://<repo>
  const ccGrc = s.match(/^codecommit::([a-z0-9-]+):\/\/(?:[^@]+@)?([^/]+)$/i);
  const parsed = ccGrc ? null : parseRemote(s);
  const ccHttp = parsed && parsed.host.match(/^git-codecommit\.([a-z0-9-]+)\.amazonaws\.com$/);
  if (ccGrc || ccHttp) {
    const region = ccGrc ? ccGrc[1] : ccHttp[1];
    const repo = ccGrc ? ccGrc[2] : parsed.path.replace(/^v1\/repos\//, '');
    return {
      provider: 'AWS CodeCommit',
      url: `https://${region}.console.aws.amazon.com/codesuite/codecommit/repositories/${encodeURIComponent(repo)}/browse?region=${region}`,
    };
  }
  if (!parsed) return null;
  let { host, path } = parsed;
  path = path.replace(/\.git$/, '').replace(/\/+$/, '');

  // Azure DevOps SSH: ssh.dev.azure.com:v3/org/project/repo
  if (host === 'ssh.dev.azure.com' || host.endsWith('vs-ssh.visualstudio.com')) {
    const [, org, project, repo] = path.split('/');
    if (!org || !project || !repo) return null;
    const base = host === 'ssh.dev.azure.com'
      ? `https://dev.azure.com/${org}/${project}/_git/${repo}`
      : `https://${org}.visualstudio.com/${project}/_git/${repo}`;
    return { provider: 'Azure DevOps', url: branch ? `${base}?version=GB${encodeURIComponent(branch)}` : base };
  }
  // GitHub/GitLab SSH-over-443 hosts map back to the web host.
  if (host === 'ssh.github.com') host = 'github.com';
  if (host === 'altssh.gitlab.com') host = 'gitlab.com';
  // Gerrit authenticated HTTP paths (`/a/<project>`): drop the auth prefix.
  if (/gerrit|review/.test(host)) path = path.replace(/^a\//, '');

  const provider = PROVIDERS.find((p) => p.test(host));
  const base = `https://${host}/${path}`;
  const suffix = branch && provider ? provider.branch(encodeURIComponent(branch).replace(/%2F/g, '/')) : '';
  return { provider: provider?.name || host, url: base + suffix };
}
