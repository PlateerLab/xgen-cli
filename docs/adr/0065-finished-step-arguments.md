# ADR-0065: 끝난 Step의 인자를 planning context에 넣기

- 상태: Accepted
- 기준일: 2026-10-27
- 변경 분류: 범용 planning context 변경. 특정 모델·작업·업무 규칙 없음.
- 기존 경계: ADR-0036(prefix 순서), ADR-0064(반복 표시·읽기 반복 거절)
- protocol 변경: planning context의 Step에 선택 필드 `arguments`가 붙는다. 값이 없으면 직렬화하지 않는다.
  profile version은 v3 그대로다.
- journal·SQLite schema 변경: 없음. 인자는 이미 저장된 material recipe에서 다시 읽는다.

## 문제

ADR-0064 뒤에도 같은 명령을 결과를 받고도 다시 실행하며 96 turn을 다 쓰는 경우가 남았다.
`~/projects/app` 아래 dirty 저장소 찾기(T4)에서 첫 `bash` 실행이 정답(저장소 이름 목록)을 내놓았는데도
모델은 같은 명령을 45번 다시 실행했다.

요청과 응답을 기록하는 loopback proxy로 실제 context를 확인했다. toolOutputs에는 stdout이 그대로 있었지만
steps에는 objective와 capability만 있고 어떤 인자로 실행했는지가 없었다. 모델이 완료를 거절한 이유를 적은
응답이 원인을 보여 준다. "두 실행 모두 저장소 목록만 출력했고 커밋되지 않은 변경 여부를 확인하는 출력이
없다." 모델은 stdout이 `git status`로 거른 결과인지 확인할 수 없어서 다시 실행했다.

같은 context를 그대로 다시 보낸 재생 실험(deepseek flash, 지점 4곳 x 4회):

| context | 원래 그대로 | 끝난 Step에 인자 추가 |
| --- | --- | --- |
| T4 2번째 call | 완료 4/4 | 완료 4/4 |
| T4 3번째 call | 완료 4/4 | 완료 4/4 |
| T4 6번째 call (반복 시작 뒤) | 계획 4/4 | 완료 4/4 |
| T4 11번째 call | 계획 4/4 | 완료 4/4 |
| T3 rename 중간 3곳 (조기 완료 확인) | 계획 9/9 | 계획 9/9 |

system prompt에 "목표가 이미 충족되면 완료하라"는 규칙을 더하는 방법도 재생했지만 오히려 계획을 더 냈다.

## 결정

1. `PlanMaterializer`에 기본 구현이 `None`인 `planned_arguments`를 둔다. material을 저장한 materializer가
   reference를 다시 열어 같은 Step·capability·material digest일 때만 정규화된 인자를 돌려준다.
2. runtime은 끝난(Completed·Failed) Step마다 store의 invocation material을 읽어 digest가 Step의
   `planInputDigest`와 같을 때만 인자를 묻는다. 정규화된 canonical 크기가 2 KiB를 넘는 인자(patch 본문,
   파일 내용)는 넣지 않는다.
3. 인자는 Step 요약의 `capability` 바로 뒤에 `arguments`로 들어간다. 끝난 Step의 인자는 바뀌지 않으므로
   prefix cache를 깨지 않는다.
4. CLI는 workspace(`xgeny.fs/*`)와 `xgeny.process/execute`를 구현한다. 정확한 파일 허용(`--allow-file`),
   discovery, terminal, web search는 아직 돌려주지 않는다.

인자는 정규화된 형태다(`workspace:primary/README.md`, `process:primary/executables/bash`). 이 형태는
capability 입력으로도 받아들여지므로 모델이 그대로 다시 써도 거절되지 않는다(e2e로 확인).

## 대안

- **system prompt에 완료 규칙 추가:** 재생에서 효과가 없었다.
- **모델이 쓴 원래 인자를 따로 저장:** journal이나 새 sidecar가 필요하다. 저장된 recipe로 충분하다.
- **process 출력에 실행한 명령을 넣기:** capability 출력 계약을 capability마다 바꿔야 한다.

## 검증

- 단위: 인자는 map에 있는 Step에만 붙고 `capability` 뒤에 온다.
- e2e: 읽기가 끝난 다음 call의 context에 그 Step의 `arguments.path`가 들어간다.
- 실사용 (deepseek flash, `--trust`, main build와 비교):

| 작업 | main | 이 변경 |
| --- | --- | --- |
| T4 dirty 저장소 (설계 사례) | 96 call 소진 2회, 3 call 완료 1회 | 2 call, 4 call 완료 |
| H5 `package.json` 있는 디렉터리 (검증 사례) | 96 call 소진 2/2 (178초, 185초) | 2 call 완료 2/2 (4초) |
| H3 디렉터리별 git 브랜치 (검증 사례) | 5 call 2/2 | 5 call 2/2 |
| H4 `async def`가 가장 많은 파일 (검증 사례) | 3 call 2/2 | 3 call 2/2 |
| T3 rename | `planner_invalid_response`로 멈춤 2/2 | 완료 1/2 (45 committed), 1/2는 읽기 반복 거절 3연속 |
| T1, T6 | | 통과 |
| T2 | | 숨은 검사 통과, 모델이 기존 테스트 하나를 깨뜨림 |

H5는 T4와 같은 모양(밖의 디렉터리를 셸로 훑는 질문)에서 같은 반복이 났고 인자를 넣자 사라졌다. H3·H4는
반복이 없던 작업이라 call 수가 같다. T3의 남은 실패는 모델 JSON의 괄호 오류와 patch 실패로, 이 결정과 무관하다.
T4·H5 답의 정확도(T4 16/24, worktree 누락)는 모델이 고른 명령의 문제로 남는다.
