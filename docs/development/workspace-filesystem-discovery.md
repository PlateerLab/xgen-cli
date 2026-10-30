# Workspace filesystem discovery

이 문서는 source/main의 workspace read-only discovery 구현과 회귀 절차다. Directory mode에 추가된
별도 승인형 mutation은 [Workspace atomic write](workspace-atomic-write.md)와
[Workspace apply patch](workspace-apply-patch.md)를 따른다. `v0.1.0-rc.4`부터 `--allow-dir` discovery와
별도 승인형 write/patch가 native release artifact에 포함된다.

## 실행 모드

기존 exact-file mode:

```bash
xgen run \
  --allow-file README.md \
  --allow-remote-model-egress \
  --allow-read \
  'README를 읽고 요약해줘.'
```

Workspace discovery mode:

```bash
xgen run \
  --workspace . \
  --allow-dir . \
  --allow-remote-model-egress \
  --allow-read \
  '프로젝트 구조를 확인하고 관련 구현을 찾아 설명해줘.'
```

`--allow-dir .`은 선택한 workspace root 전체를 다섯 read-only Capability로 관찰할 후보 경계를 제공한다.
같은 mode에서 `write-atomic`과 `apply-patch`도 광고되지만 실제 mutation은 별도 `--allow-write`가 있어야 한다.
Planner는 immutable input schema에서 `.` 규칙을 받고, 별도 `planningConstraints`에서 caller가 허용한
directory/file 목록을 받는다. Constrained provider prompt는 이를 후보 제한으로 따르되 권한으로
간주하지 않는다. 또한 미래 observation을 placeholder로 참조하지 않고, 현재 context에서 argument가
확정된 Step 하나만 plan한 뒤 receipt-completed tool output을 다음 turn에서 받는다. 실제 실행 경계는
동일 catalog를 component 단위로 다시 검사한다.

| Capability | 동작 |
| --- | --- |
| `xgeny.fs/list-directory@1.0.0` | 한 directory의 직계 entry 목록 |
| `xgeny.fs/stat@1.0.0` | regular file size 또는 directory kind |
| `xgeny.fs/search-text@2.0.0` | workspace 내부 `.gitignore`·`.ignore`를 따르는 case-sensitive literal UTF-8 검색 |
| `xgeny.fs/read-text@1.0.0` | bounded UTF-8 file content 읽기 |
| `xgeny.fs/read-text-lines@2.0.0` | 8 MiB 이하 UTF-8 file의 줄 범위 읽기와 file 전체 digest ([ADR-0053](../adr/0053-bounded-line-range-text-read.md), [ADR-0056](../adr/0056-large-file-patch.md)) |
| `xgeny.fs/write-atomic@1.0.0` | 별도 승인 아래 UTF-8 file create/atomic replace |
| `xgeny.fs/apply-patch@2.0.0` | 별도 승인 아래 8 MiB 이하 기존 UTF-8 file의 strict contextual edit |

더 좁은 범위는 directory를 반복 지정하고 필요한 exact file만 추가한다.

```bash
xgen run \
  --workspace . \
  --allow-dir src \
  --allow-dir tests \
  --allow-file Cargo.toml \
  --allow-remote-model-egress \
  --allow-read \
  '구현과 테스트 구조를 비교해줘.'
```

미완료 Run은 동일한 workspace와 catalog를 다시 제공한다.

```bash
xgen resume run-0123456789abcdef0123456789abcdef \
  --workspace . \
  --allow-dir src \
  --allow-dir tests \
  --allow-file Cargo.toml \
  --allow-remote-model-egress \
  --allow-read
```

완료된 Run은 기존과 같이 다른 입력 없이 offline replay한다.

## Durable material과 state

Discovery mode는 Run directory에 두 SQLite file을 둔다.

```text
runs/<run-id>/
├── manifest.json
├── run.sqlite3
├── materials.sqlite3
└── run.lock
```

`run.sqlite3`는 Core journal/WorkGraph/Receipt/tool output을 보유하고 schema 8을 유지한다.
`materials.sqlite3` schema 1은 모델이 선택한 dynamic path, search query와 mutation content를 opaque
digest reference로 복원하기 위한 CLI-owned recipe catalog다. 둘 다 binary에 embedded된 SQLite library를 사용하므로
SQLite 프로그램이나 server를 설치하지 않는다.

Manifest, WorkGraph event, Receipt와 status output에는 raw write/patch content가 들어가지 않는다. 하지만 private
material DB에는 path/query/content가, Run DB에는 goal과 successful output이 존재하므로 state root를 민감
데이터로 취급한다. Completed summary replay는 두 DB 중 material catalog와 workspace가 없어도 된다.

## Limit과 해석

- authorization catalog: file/directory 합계 64, run-local planner constraint 16 KiB
- list: directory scan 4,096, 반환 512
- list/stat/search output: digest 포함 canonical JCS 최대 128 KiB
- search: query 최대 64 Unicode scalar이면서 256 UTF-8 bytes, raw visited entry
  4,096, file 256 KiB, ignore 파일 읽기를 포함한 실제 read aggregate 8 MiB, match 128,
  preview 512 bytes
- read-text/write-atomic: file 64 KiB
- apply-patch: source/result와 old/new 합 각각 64 KiB, edit 32개
- discovery Run: model turn 8, provider reservation 16, planned/tool Step 8, context 512 KiB

`truncated=true`는 검색 대상 범위 안에서 결과가 complete하지 않다는 뜻이다. Search는 limit 도달뿐 아니라
VCS metadata, invalid UTF-8, oversized 또는 열 수 없는 file을 건너뛰어도 이 값을 세운다. Ignore 규칙으로
제외한 경로는 검색 대상이 아니므로 이것만으로 `truncated=true`가 되지는 않는다. Regex는 지원하지 않는다.

### 검색의 ignore 규칙

- workspace root부터 요청 directory까지의 `.gitignore`·`.ignore`와, 탐색 중 만나는 하위 directory의
  같은 파일을 읽는다. `.git` 유무와 무관하게 적용한다.
- Git glob 문법, `!` 예외, directory 전용 규칙, `/`로 시작하는 상대 root 규칙을 지원한다.
  같은 종류는 더 가까운 파일과 뒤쪽 줄이 우선이고, `.ignore`가 `.gitignore`보다 우선한다.
  제외한 directory 안에는 들어가지 않으므로 그 안의 예외 규칙은 적용되지 않는다.
- 규칙 해석은 ripgrep의 `ignore` crate를 사용한다. 파일 탐색과 규칙 파일 읽기는 기존 opened workspace
  handle을 사용하며 symlink를 따라가지 않는다. workspace 밖의 부모 파일, 전역 Git 설정,
  `.git/info/exclude`, `.rgignore`는 읽지 않는다. 숨김 파일을 이름만으로 제외하지 않는다.
- ignore 파일도 파일당 256 KiB와 검색 전체 8 MiB 읽기 예산을 따른다. 존재하는 규칙 파일이 symlink,
  unreadable, oversized, invalid UTF-8이거나 문법을 해석할 수 없으면 검색을 실패시킨다.
  규칙을 버리고 더 넓은 범위를 검색하지 않는다.
- ignore는 검색 필터다. 별도 허용된 `read-text`, `read-text-lines`, `list-directory`의 접근 권한은
  바뀌지 않는다. 제외한 파일을 읽어야 하면 정확한 경로로 읽거나 `.ignore`에 예외를 둔다.
- `search-text@1.0.0`에서 Definition이 바뀌므로 이전 binary에서 시작한 미완료 workspace Run은
  새 binary로 재개하면 `configuration_mismatch`로 거절된다. 새 Run을 시작해야 한다.
  완료 Run의 offline replay는 기존 기록을 사용한다.

결정과 검증 근거는 [ADR-0076](../adr/0076-ignore-aware-text-search.md)을 따른다.

## 검증

```bash
cargo test --locked -p xgen-adapter-filesystem --all-targets
cargo test --locked -p xgen-cli --test workspace_discovery
cargo test --locked -p xgen-cli --test public_run_resume
cargo run --locked --quiet -p xgen-cli -- protocol check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

`workspace_discovery` E2E는 실제 child process와 loopback fake provider로 다음 두 경로를 검증한다.

```text
list -> search -> stat -> read -> completion -> workspace/material 삭제 -> offline replay

model plan -> approval pause -> process 종료
  -> local-only resume에서 dynamic search material 복원/실행
  -> remote resume에서 durable output 전달 -> completion

write plan -> 별도 write approval pause -> process 종료
  -> local-only resume에서 content 복원/atomic write
  -> remote resume에서 content 없는 durable output 전달 -> completion
```

실제 Qwen의 structured capability 호출과 동적 경로 continuation은 별도 ignored release test로
검증한다. Search preview와 read 결과를 구분하기 위해 locator와 최종 sentinel을 서로 다른 줄에 둔 임시
workspace를 만들고, 모델이 지시된 `list-directory`, `search-text`, `stat`, `read-text`를 모두 실행한
뒤에만 sentinel을 exact summary로 완성하는지 확인한다. Accepted Plan마다 dependency 없는 Step 하나만
있는지도 검사한다. 완료 후 tunnel, workspace와 dynamic material catalog를 제거하고 offline replay와
journal 불변까지 검사한다.

```bash
XGEN_LIVE_CONFIRM=xgen-go50902-workspace-discovery-v1 \
XGEN_LIVE_KNOWN_HOSTS_FILE=/absolute/path/to/dedicated_known_hosts \
XGEN_LIVE_OPENAI_BASE_URL=http://127.0.0.1:18000/v1 \
cargo test --locked --release -p xgen-cli \
  --test live_go50902_public \
  public_cli_workspace_discovery_and_offline_replay \
  -- --ignored --exact
```

전용 known-host file에는 `HostKeyAlias=go50902`로 별도 경로에서 검증한 public host key가 있어야 한다.
Test가 tunnel lifecycle을 직접 소유하므로 다른 terminal에서 같은 local port의 tunnel을 미리 열지 않는다.
`--nocapture`, `--show-output`, shell tracing과 `tee`도 사용하지 않는다. 상세한 보안 전제와 exact-file
gate 및 discovery gate의 clean-SHA 증거는
[Public local run/resume](public-local-run-resume.md)를 따른다.

결정 근거와 보장 한계는 [ADR-0024](../adr/0024-workspace-filesystem-discovery.md)를 따른다.
