# ADR-0054: 실패한 Step의 값 없는 원인 분류를 planner에 전달

- 상태: Accepted
- 기준일: 2026-10-25
- 변경 분류: 범용 runtime 계약. 특정 도구·파일·업무 규칙 없음.
- 기존 경계: ADR-0022, ADR-0038, ADR-0052, ADR-0053
- journal 변경: `effect_failed` 이벤트와 Step projection에 선택 필드 `failureCode` 추가. SQLite schema는 그대로다.
- planner prompt 변경: 없음. planning context의 실패한 Step 요약에 선택 필드 `failureCode`가 붙는다.

## 문제

ADR-0052 이후 확정된 읽기 실패 뒤에도 계획은 이어진다. 하지만 모델은 그 Step이 `failed`라는 것만
보고 이유는 모른다. adapter는 고정 분류의 SHA-256 digest만 남기고, ADR-0022·0052는 원인 분류
전달을 범위 밖으로 남겼다.

실사용에서 모델은 이유를 추측하느라 같은 실패를 반복했다. 큰 파일 읽기에서는 같은 `read-text`를
두 번 실패한 뒤 `stat`으로 크기를 확인하고서야 줄 범위 읽기로 바꿨다. 없는 파일을 물은 요청에서는
같은 경로의 `read-text`·`stat`을 3–17번 반복했고, 한 번은 턴 예산을 다 써서 답하지 못했다.

## 결정

### 1. 닫힌 분류 `StepFailureCode`

adapter의 확정 실패(`AdapterExecutionObservation::Failed`)는 evidence digest와 함께 `failure_code`를
반드시 낸다. 분류는 workgraph의 닫힌 enum이다.

| code | 뜻 |
| --- | --- |
| `not_found` | 경로가 없다. 중간 경로가 directory가 아닌 경우를 포함한다. |
| `unavailable` | 있지만 confined workspace에서 열 수 없다. symlink, 권한, 이식 불가 이름 등 |
| `not_a_file` | regular file이 필요했다. |
| `not_a_directory` | directory가 필요했다. |
| `too_large` | 입력이나 대상이 capability 크기 상한을 넘었다. |
| `not_utf8` | UTF-8 text가 아니다. |
| `limit_exceeded` | bounded scan 또는 output 상한에 닿아 관찰을 끝낼 수 없었다. |
| `precondition_conflict` | 관찰 뒤 대상이 바뀌어 expected digest가 맞지 않는다. |
| `patch_context_missing` | edit의 원문이 대상에 없다. |
| `patch_context_ambiguous` | edit의 원문이 대상에 여러 번 있다. |
| `patch_edits_overlap` | edit끼리 겹친다. |
| `io_error` | 읽기·쓰기 중 실패했다. |

code는 종류만 나타낸다. 경로, 크기, 내용, OS 오류 문자열을 담지 않는다. 그래서 planner에 보내도
ADR-0038의 redaction 경계가 넓어지지 않는다. 새 code는 enum에 추가하는 방식으로만 늘린다. 업무
의미를 가진 code는 넣지 않는다.

symlink를 따르지 않고 열 때 일부 플랫폼은 "not a directory"를 보고한다. 이 경우 이름의
`symlink_metadata`를 확인해 `unavailable`로 분류한다. 존재하는 symlink를 `not_found`로 말하지 않는다.

### 2. journal과 projection

`effect_failed` 이벤트에 `failureCode`를 선택 필드로 기록한다. reducer는 이를 Step의 `failureCode`에
복사한다. 값이 없으면 직렬화하지 않는다. 따라서 이 변경 이전 이벤트와 projection은 바이트 단위로
그대로 replay된다. verification 실패처럼 adapter 실패가 아닌 경로는 이벤트를 바꾸지 않는다.

### 3. planning context

`failed` 상태 Step 요약에만 `failureCode`를 붙인다. adapter가 기록한 code를 그대로 쓴다. Receipt가
기록된 뒤 verification에서 실패한 Step은 `verification_failed`다. 둘 다 없으면, 곧 이 변경 이전에
기록된 실패라면 필드를 생략한다. 다른 상태에는 붙이지 않는다.

field가 붙는 것은 실패한 Step이 있을 때뿐이다. 그래서 실패가 없는 Run의 planning context는 바이트
단위로 그대로다. context profile 이름(`xgeny.planning-context/v2`)과 prompt는 바꾸지 않는다.

### 4. 사용자 표시

read-only가 아닌 Step의 실패로 Run이 멈추면(ADR-0052) 그 Step의 code를 사용자에게도 보인다.

- `xgen run`은 `XGEN_REJECTED` 다음 줄에 `XGEN_STEP_FAILURE run_id=<id> code=<code>`를 출력한다.
- 대화형 `xgen`은 `A step failed: <code>.`를 출력한다.

기존 `reason=failed_work`는 그대로다.

## 결과

- 모델은 실패 이유에 맞춰 다음 단계를 고른다. 예를 들어 `too_large`이면 줄 범위 읽기로,
  `not_found`이면 목록·검색으로 넘어간다.
- 쓰기 실패는 여전히 Run을 멈춘다(ADR-0052). 실패 이유는 이제 사용자에게 보인다. 확정 실패한
  쓰기 뒤 계획을 이어 가는 것은 별도 결정이다.
- 이 변경 이전 binary는 `failureCode`가 있는 journal을 읽지 못한다. 기존 journal은 새 binary에서
  그대로 읽힌다.
- 같은 실패의 failure evidence digest 일부가 바뀐다. 예를 들어 없는 경로는 `open` 대신 `not-found`
  분류다. 이미 기록된 digest는 다시 계산하지 않는다.

## 검증

- adapter 단위:
  - read-text·query·apply-patch·write-atomic의 실패별 code
  - 중간 경로가 파일이면 `not_found`
  - 대상이 파일이면 `not_a_directory`
  - symlink 경로는 기존처럼 거절
- workgraph: `failureCode`가 없는 이벤트·projection의 직렬화가 이전과 같고, 있으면 Step에 남는다.
- runtime: 확정 실패의 code가 journal을 거쳐 Step에 남는다.
- e2e:
  - 64 KiB 초과 읽기 실패 뒤 다음 planning context의 Step에 `failureCode: too_large`가 있다.
  - stale digest patch로 멈춘 `xgen run`이 `XGEN_STEP_FAILURE ... code=precondition_conflict`를 출력한다.
- 실사용: 대화형 `xgen`, 같은 planner model(default profile), 사례마다 이전 main과 이 변경을 3회씩
  실행했다. 두 사례 모두 이 ADR의 설계에 쓰지 않은 저장소다.

  | 사례 | 실패한 Step 수, 이전 | 실패한 Step 수, 이후 | 결과 |
  | --- | --- | --- | --- |
  | 113 KB 파일의 동작 설명 | 2, 2, 2 | 1, 1, 1 | 6회 모두 정답 |
  | 없는 `CHANGELOG.md` 요약 요청 | 5, 17, 3 | 1, 2, 1 | 이전 1회는 턴 예산 소진으로 답 없음, 이후 3회 모두 "없다"고 정확히 답함 |
