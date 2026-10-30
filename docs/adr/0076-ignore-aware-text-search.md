# ADR-0076: workspace 텍스트 검색이 로컬 ignore 규칙을 따른다

- 상태: Accepted
- 기준일: 2026-10-30
- 변경 분류: 범용 filesystem capability 계약. 특정 언어·의존성 폴더 이름을 하드코딩하지 않는다.
- 계약 변경: `xgeny.fs/search-text` 1.0.0 → 2.0.0. 입력·출력 schema, journal과 planner envelope 형식은 그대로다.

## 문제

기존 검색은 `.git`·`.hg`·`.svn`만 제외한다. 의존성·생성 파일이 먼저 128개 match를 채우면
실제 소스에 닿기 전에 `truncated=true`로 끝난다. 모델이 범위를 좁히지 않으면 필요한 소스를 놓친다.
이 문제는 기존 H6·H7 기록에 이어, 아래 두 독립 layout에서도 모델 호출 없이 재현했다.

## 결정

1. 파일 탐색은 기존 `cap-std`와 nofollow open을 유지한다. ripgrep의 `ignore` crate는
   `GitignoreBuilder::add_line`과 `Gitignore::matched`를 통한 glob 해석에만 쓴다.
   `WalkBuilder`, `GitignoreBuilder::add`, 전역 설정 자동 발견처럼 ambient I/O를 하는 API는 쓰지 않는다.
2. workspace root부터 요청 경로의 부모까지 규칙을 쌓고, 요청 directory와 각 하위 directory의
   `.gitignore`·`.ignore`를 읽는다. 규칙은 해당 directory에 상대적으로 적용한다.
   `.ignore`가 모든 `.gitignore`보다 우선하고, 같은 종류는 더 가까운 directory의 규칙이 우선한다.
   같은 파일 안에서는 마지막으로 맞는 줄이 우선한다. Git 저장소가 없어도 두 파일을 적용한다.
3. 무시된 directory는 들어가지 않는다. 그 안의 규칙으로 부모의 directory 제외를 취소할 수 없다.
   요청 directory 자체는 명시된 탐색 시작점이고, 필터는 그 아래 entry에 적용한다.
   workspace 밖의 ignore 파일, Git 전역 설정·`.git/info/exclude`, `.rgignore`는 지원 범위에서 뺀다.
   숨김 파일·폴더를 이름만으로 제외하지 않는다.
4. ignore 파일은 기존 workspace handle에서 component별 nofollow로 연 regular file만 읽는다.
   이 규칙 파일 읽기도 search capability의 동작에 포함되며, 하위 directory를 요청해도 workspace 내부의
   상위 규칙 파일을 해석한다. 규칙 원문은 별도 출력이나 journal 필드에 넣지 않는다.
5. 규칙 파일은 파일당 256 KiB, 실제 read는 검색 전체 8 MiB 예산에 합산한다.
   없는 파일만 건너뛰고, 존재하지만 읽거나 해석할 수 없는 파일은 검색 실패로 처리한다.
   불완전한 규칙으로 검색 범위를 조용히 넓히지 않는다. UTF-8 BOM과 CRLF는 처리한다.
6. ignore로 제외한 경로는 검색 대상 범위 밖이므로 `truncated`를 세우지 않는다.
   기존 entry·match·byte 한도, VCS metadata·binary·oversized·unopenable 경로의 불완전성 표시는 유지한다.
7. ignore는 접근 통제가 아니다. 허용된 정확한 경로의 read와 directory 목록은 기존대로다.
   제외한 경로의 검색이 필요하면 `.ignore`의 예외 규칙을 쓸 수 있다.

## 호환성

Definition의 버전과 summary가 바뀌어 workspace execution profile digest가 바뀐다.
이전 binary에서 시작한 미완료 workspace Run은 새 binary에서 `configuration_mismatch`로 재개가 거절된다.
완료한 Run은 저장된 Receipt·ToolOutput으로 offline replay하고 현재 ignore 파일을 다시 읽지 않는다.
대화형·headless 모두 같은 adapter를 사용한다. 지침 파일 읽기(ADR-0075)와는 별개다.

## 검증

모델 호출 없는 결정적 filesystem 회귀이며 실제 LLM의 작업 완료율·토큰 비용 개선을 측정한 것은 아니다.

| 사례 | 이전 구현 | 변경 후 |
| --- | --- | --- |
| 설계: Python `.gitignore`의 `.venv/`, 의존성 129개 match와 소스 1개 | 128개에서 잘림, 소스 누락 | 소스 1개만 반환, 잘림 없음 |
| 설계: JavaScript `.ignore`의 `node_modules/`, 의존성 129개 match와 소스 1개 | 128개에서 잘림, 소스 누락 | 소스 1개만 반환, 잘림 없음 |
| 보류 검증: Rust build glob과 `!` 예외, 생성 파일 150개 match | 이전 구현 실측 없음 | CLI E2E로 예외·소스 2개만 반환, 잘림 없음 |

추가 adapter 회귀는 workspace-local 상위 규칙·중첩 우선순위·형제 scope 복원·앵커·CRLF/BOM,
제외 directory 가지치기, workspace 밖 규칙 미참조, 규칙 읽기 예산, invalid/oversized 규칙 거절,
Unix symlink 거절을 확인한다. 보류 CLI E2E는 무시된 파일의 명시적 읽기와 ignore 변경 뒤 완료 replay도 확인한다.

로컬 `cargo test --workspace --locked`, workspace Clippy·fmt, protocol check, 공개 문서·배포 workflow 검사와
전체 지원 target의 third-party license notice 생성을 통과했다. 독립 diff 검토에서 제기된 상대 경로·앵커와
읽기 예산 경계는 crate 소스와 추가 회귀로 재확인했다.

브랜치 전체 [CI 7개 job](https://github.com/PlateerLab/xgen-cli/actions/runs/41035044774)이 코드 commit
`04036d1`에서 통과했다. Linux x86-64·ARM64, macOS Intel·Apple Silicon, Windows x64의 release 빌드와
설치 smoke를 포함한다. Intel macOS job은 기존 CI 정책대로 테스트를 생략하고 macOS 테스트는 Apple Silicon에서 실행한다.

## 참고

- [ignore GitignoreBuilder](https://docs.rs/ignore/0.4.34/ignore/gitignore/struct.GitignoreBuilder.html)
- [ripgrep 자동 필터링과 우선순위](https://github.com/BurntSushi/ripgrep/blob/master/GUIDE.md#automatic-filtering)
