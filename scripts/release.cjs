#!/usr/bin/env node
/**
 * 一键发版：打印摘要 → 二次确认 → bump 版本 → commit → (tag 已存在时确认清理) → 打 tag → 推送。
 * 推送触发 "Release" workflow，由 GitHub Actions 完成构建与发布。
 *
 * 确认规则（参考 wmdebugger/scripts/release.js）：
 *   - tag 已存在（本地或远程）：y/N 确认后删除旧 tag 再重打并推送
 *   - tag 不存在：回车确认发版，Ctrl+C 取消
 *
 * 用法：
 *   npm run release                # 用当前版本，tag & push
 *   npm run release patch          # bump patch
 *   npm run release minor|major
 *   npm run release 0.2.0          # 指定版本号
 */
const { execSync } = require('child_process');
const path = require('path');
const fs = require('fs');
const readline = require('readline');

const PKG_JSON = 'package.json';
const TAG_PREFIX = 'v';
const CHANGELOG = 'CHANGELOG.md';
const WORKFLOW_NAME = 'Release';

const SEMVER_RE = /^\d+\.\d+\.\d+$/;
const BUMP_TYPES = ['patch', 'minor', 'major'];

function usage(msg) {
  if (msg) console.error(`error: ${msg}\n`);
  console.error('Usage: node scripts/release.js [<version>|patch|minor|major]');
  console.error('  no arg           : use current version, tag & push');
  console.error('  patch|minor|major: bump version, commit, tag & push');
  console.error('  X.Y.Z            : set version, commit, tag & push');
  process.exit(1);
}

function run(cmd, opts = {}) {
  console.log(`> ${cmd}`);
  return execSync(cmd, { stdio: 'inherit', ...opts });
}

function sh(cmd) {
  return execSync(cmd, { encoding: 'utf8' }).trim();
}

function commandSucceeds(cmd) {
  try {
    execSync(cmd, { stdio: 'ignore' });
    return true;
  } catch {
    return false;
  }
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function writeJson(file, data) {
  fs.writeFileSync(file, JSON.stringify(data, null, 2) + '\n');
}

function bumpVersion(version, type) {
  const [maj, min, pat] = version.split('.').map(Number);
  if (type === 'major') return `${maj + 1}.0.0`;
  if (type === 'minor') return `${maj}.${min + 1}.0`;
  return `${maj}.${min}.${pat + 1}`;
}

function isWorkingTreeClean() {
  return sh('git status --porcelain').length === 0;
}

function tagExists(tag) {
  return commandSucceeds(`git rev-parse -q --verify "refs/tags/${tag}"`);
}

function remoteTagExists(tag) {
  return commandSucceeds(`git ls-remote --exit-code origin "refs/tags/${tag}"`);
}

function deleteExistingTag(tag) {
  if (remoteTagExists(tag)) {
    run(`git push origin --delete "${tag}"`);
  }
  if (tagExists(tag)) {
    run(`git tag -d "${tag}"`);
  }
}

// the Release workflow pulls release notes from the "## <version>" section of CHANGELOG.md
function changelogHasVersion(file, version) {
  if (!fs.existsSync(file)) return false;
  const lines = fs.readFileSync(file, 'utf8').split('\n');
  return lines.some((l) => l.trim().split(/\s+/).slice(0, 2).join(' ') === `## ${version}`);
}

function main() {
  const arg = process.argv[2];
  const root = path.resolve(__dirname, '..');
  // 版本以根目录 package.json 为准（tauri.conf.json 的 version 指向 ../../../package.json）
  const pkgRel = PKG_JSON;
  const pkgPath = path.join(root, PKG_JSON);
  const pkg = readJson(pkgPath);

  const currentVersion = pkg.version;
  if (!currentVersion || !SEMVER_RE.test(currentVersion)) {
    console.error(`error: cannot resolve a valid version from ${PKG_JSON}`);
    process.exit(1);
  }

  let nextVersion = currentVersion;
  let versionChanged = false;

  if (arg) {
    if (BUMP_TYPES.includes(arg)) {
      nextVersion = bumpVersion(currentVersion, arg);
      versionChanged = true;
    } else if (SEMVER_RE.test(arg)) {
      nextVersion = arg;
      versionChanged = nextVersion !== currentVersion;
    } else {
      usage(`invalid version/bump "${arg}"`);
    }
  }

  const tag = `${TAG_PREFIX}${nextVersion}`;

  // ---------- 前置检查（任何破坏性操作之前） ----------
  let status;
  try {
    status = sh('git status --porcelain');
  } catch {
    console.error('✗ 不在 git 仓库中');
    process.exit(1);
  }
  if (versionChanged && status) {
    console.error('✗ 工作区有未提交改动，请先 commit 或 stash：');
    console.error(status);
    process.exit(1);
  }

  const branch = sh('git rev-parse --abbrev-ref HEAD');
  if (branch === 'HEAD') {
    console.error('✗ 当前处于 detached HEAD，请先 checkout 到分支');
    process.exit(1);
  }

  // ---------- 打印发版摘要 ----------
  console.log('────────────────────────────────────────');
  console.log(`  当前版本:  ${currentVersion}`);
  console.log(`  发布版本:  ${tag}`);
  console.log(`  分支:      ${branch}`);
  console.log('────────────────────────────────────────');

  if (!changelogHasVersion(path.join(root, CHANGELOG), nextVersion)) {
    console.warn(`warning: ${CHANGELOG} has no "## ${nextVersion}" section — release notes will fall back to "Release ${nextVersion}"`);
  }

  // ---------- tag 状态检测：确认前说明将删除已有还是新建 ----------
  const tagLocal = tagExists(tag);
  const tagRemote = remoteTagExists(tag);
  const tagWhere = [
    tagLocal ? '本地' : null,
    tagRemote ? '远程' : null,
  ].filter(Boolean).join('、');

  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const ask = (prompt) => new Promise((resolve) => rl.question(prompt, resolve));

  (async () => {
    let needClean = false;
    if (tagLocal || tagRemote) {
      console.log(`  tag 状态:  已存在（${tagWhere}）→ 将删除后重打`);
      const ans = await ask(`⚠ tag ${tag} 已存在（${tagWhere}）。删除后重新打 tag 并推送？[y/N] `);
      if (ans.trim().toLowerCase() !== 'y') {
        console.log('✗ 已取消，未删除任何 tag，发版中止');
        process.exit(1);
      }
      needClean = true;
    } else {
      console.log('  tag 状态:  新建');
      await ask('回车确认发版，Ctrl+C 取消...');
    }

    // ---------- 确认通过后才执行写操作 ----------
    if (versionChanged) {
      if (!isWorkingTreeClean()) {
        console.error('error: working tree not clean — commit or stash before releasing');
        process.exit(1);
      }

      pkg.version = nextVersion;
      writeJson(pkgPath, pkg);

      run(`git add "${pkgRel}"`);
      run(`git commit -m "chore: release v${nextVersion}"`);
    }

    if (needClean) {
      deleteExistingTag(tag);
      console.log(`✓ 已清理旧 tag ${tag}`);
    }

    run(`git tag -a "${tag}" -m "SViewer v${nextVersion}"`);
    run('git push');
    run(`git push origin "${tag}"`);

    console.log('');
    console.log(`✓ pushed tag ${tag} — workflow "${WORKFLOW_NAME}" triggered`);
    rl.close();
    process.exit(0);
  })();
}

main();
