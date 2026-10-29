# ADR-0075: 대화형 세션은 사용자·프로젝트 지침 파일을 goal에 붙인다

- 상태: Accepted
- 기준일: 2026-10-30
- 변경 분류: 범용 대화형 host 입력. planning context 필드·envelope·journal 형식 변경 없음. 모델에 가는 goal
  내용이 바뀌므로 사용자 확인을 받았다(2026-10-30).

## 문제

xgen은 저장소의 `AGENTS.md`·`CLAUDE.md`도, 사용자 전역 지침도 읽지 않았다. Claude Code·Codex는 둘 다 읽으므로
같은 저장소에서 xgen만 테스트 방법, 응답 언어, 코드 규칙을 모른다. 모델이 스스로 파일을 열어 볼 수는 있지만
대개 열지 않는다(아래 main 결과).

## 결정

1. 대화형 세션(`xgen`)은 시작할 때 한 번 다음 파일을 읽는다.
   - 사용자 파일: xgen 설정 root의 `AGENTS.md`(Linux `~/.config/xgen-cli/AGENTS.md`, `XGEN_CONFIG_HOME`을 따른다).
   - 프로젝트 파일: workspace root의 `AGENTS.md`, 없으면 `CLAUDE.md`. 하위 디렉터리와 `@import`는 따라가지 않는다.
   - 없거나, 파일이 아니거나, 비어 있으면 건너뛴다. NUL은 지운다.
2. 두 파일을 합쳐 32 KiB까지 읽는다. 사용자 파일이 먼저다. 한 파일이 잘리면 뒤 파일은 읽지 않고 `[truncated]`를
   붙인다. 잘린 문자는 replacement 문자로 만들지 않는다.
3. 지침은 `<instructions source="...">` 블록으로 Run마다 goal 뒤에 붙는다. 앞에 고정 문장을 둔다: 지침은 일하는
   방식에 대한 안내이고 goal이 작업이며, 둘이 어긋나면 goal을 따른다. 프로젝트 지침이 사용자 지침보다 구체적이다.
4. ADR-0063 환경 안내를 먼저 붙이고 지침을 뒤에 붙인다. 합친 goal이 `MAX_GOAL_BYTES`를 넘으면 뒤의 지침 블록을
   잘라 맞춘다(Windows 16 KiB에서도 Run이 거절되지 않는다).
5. 시작 화면에 `instructions: ~/.config/xgen-cli/AGENTS.md (1 KiB), AGENTS.md (3 KiB)`처럼 읽은 파일을 보인다.
   지침은 goal의 일부이므로 기존 model 승인(`goal, session context ...`)을 받아야 모델에 간다.
6. goal과 함께 journal에 남으므로 재개한 Run은 시작할 때의 지침으로 계획한다. 세션 중 파일을 고치면 다음 세션부터
   반영된다.
7. `xgen run`(headless)은 읽지 않는다. 스크립트·평가 입력이 실행 위치의 파일로 바뀌지 않게 한다.

## 검증

DeepSeek profile, `--trust`, 사례마다 main(지침 안 읽음)과 새 빌드 각 2회. 사용자 파일은 모든 새 빌드 실행에
적용됐다(`[xgen]`으로 시작, 새 파일 만들지 않기).

| 사례 | 지침 | 규칙 | main | 새 빌드 |
|------|------|------|------|---------|
| A 설계(Python) | 프로젝트 AGENTS.md | 테스트는 `sh scripts/test.sh`로만 | 0/2 (unittest 직접) | 2/2 |
| A | 프로젝트 AGENTS.md | 마지막 줄 `-- done` | 0/2 | 2/2 |
| B 설계(JS) | CLAUDE.md만 있음 | 한국어 요청에도 영어로 답 | 0/2 | 2/2 |
| B | CLAUDE.md | export 함수마다 JSDoc 한 줄 | 0/2 | 새 함수만 2/2, 기존 함수 0/2 |
| C 보류(사용자 파일) | 사용자 AGENTS.md | 답을 `[xgen]`으로 시작 | 0/2 | 2/2 (전체 새 빌드 10/10) |
| C | 사용자 AGENTS.md | 새 파일 대신 기존 파일 수정 | 2/2 | 2/2 (구분 안 됨) |
| E 보류(실제 저장소 12 KiB CLAUDE.md) | 프로젝트 CLAUDE.md | 소문자 generic 타입 힌트 | 1/2 | 2/2 |
| E | 프로젝트 CLAUDE.md | logging 사용(stdout 금지) | 1/2 | 2/2 |

- D(같은 저장소, "화면에 출력하는 함수")는 main·새 빌드 모두 `print`를 썼다(0/2, 0/2). goal이 화면 출력을
  요구해 지침과 어긋나므로 고정 문장대로 goal을 따른 결과로 본다.
- 실행 시간과 Step 수는 두 빌드가 같은 수준이었다(사례당 3~11초).
- 단위 테스트: 사용자 파일 우선과 AGENTS.md 우선, 없는·빈·디렉터리 파일 건너뛰기, 공유 상한과 문자 경계 자르기,
  goal 상한 안에서 붙이기.

## 남은 일

- 지침은 매 model call 입력에 들어간다. 12 KiB 한국어 파일은 call당 수천 token이지만 goal은 접두부라 대부분
  cache에 걸린다. 긴 작업에서 비용을 따로 재지 않았다.
- 하위 디렉터리 지침, `@import`, 세션 중 다시 읽기(`/instructions reload`)는 필요가 확인되면 다룬다.
