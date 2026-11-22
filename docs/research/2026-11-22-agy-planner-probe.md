# agy를 통한 Gemini 계획 전용 연결 실험

2026-11-22. **별도 모델 API key 없이 로그인한 공식 agy CLI로 제안을 받고, 실제 실행은 xgen이 맡는
경로를 확인했다.** 다만 agy의 실행기·내부 도구를 완전히 제거한 raw model API 연결은 아니다.
현재 판정은 일반 설계 후보 `ADOPT_WITH_GATES`이며, 정식 provider·runtime 계약·배포본은 변경하지 않았다.

## 무엇을 실제로 실행했나

- 로컬 debug xgen 빌드와 Gemini `gemini-3.8-flash-low`를 사용했다. 공개 release artifact 품질 평가가 아니다.
- 별도 임시 workspace에 custom agent를 정의했다. `excludeDefaultComponents: true`, `tools: []`,
  `inheritMcp: false`, `inheritCustomizations: false`, 빈 rules/agents/hooks/preloadSkills를 설정했다.
- 프로세스별 설정으로 테스트 workspace만 신뢰하고, 문서화된 command/read/write/URL/MCP deny 규칙을
  적용했다. 추가 AI credit fallback은 끈 설정이다. 사용자 credential이나 기존 private memory를 복사하지 않았다.
- agy는 headless `stream-json`으로 요청마다 새 conversation을 사용했다. 파일 읽기·변경·명령 실행을
  직접 하지 말고, 호스트가 제공한 context에서 제안 JSON 하나만 반환하도록 했다.
- 임시 loopback 연결기가 xgen의 원래 system instructions·planning context를 agy에 전달하고,
  반환된 제안을 OpenAI-compatible 응답 envelope로 돌려줬다. xgen의 기존 검증·승인·실행·Receipt를 사용했다.
- 연결기는 native tool event, workspace 변경, 잘못된 상태나 JSON을 관찰하면 응답을 실패 처리하도록
  구성했다. 관찰 후 거절이므로 agy 내부 동작을 사전에 모두 막았다는 보장은 아니다.
- 합성 파일만 제공했다. agy workspace와 xgen의 실제 실행 workspace는 분리했다.

## 실제 연결 결과

| 사례 | xgen이 수행한 일 | 독립 판정 | agy CLI 호출 | xgen Receipt |
| --- | --- | --- | --- | --- |
| 저장소 질문 | 파일 읽기 → 내용에 근거한 답변 | 정확한 값, 파일 변경 없음 | 2 | 1 |
| 승인 후 쓰기 | 쓰기 승인 전 정지 → 같은 Run 재개 → 파일 생성 | 승인 전 파일 없음, 승인 뒤 exact content | 2 | 1 |
| 실패 복구 | 테스트 실패 → 소스 읽기·patch → 같은 테스트 성공 | 테스트 파일 보존, 수정 결과 재검증 | 5 | 4 |

세 사례 모두 완료·독립 검사·정상 종료를 충족했다. 합계 CLI 호출 9회, xgen Receipt 6개, Unknown·동일
effect 중복 시작 0건이었다. **이 9회에서 관찰한 agy native tool event는 0개**이고 agy workspace도
변하지 않았다. 읽기 결과와 실행 결과를 다음 제안에 다시 넣은 주체는 xgen context였다.

agy가 보고한 9회 합산 usage는 입력78,675·출력1,513·총80,188 tokens, cache-read0이었다.
이는 CLI 보고 값이며 별도 provider 원시 응답·invoice 검증은 아니다. USD 비용과 서버 내부 model request
횟수는 UNKNOWN이다. CLI 호출 9회를 실제 inference request 9회로 단정하지 않는다.

각 사례 1회인 연결 smoke다. 정확도 향상·범용 작업 완료율·Claude Code/Codex 대비 우위·전체 도구 호환성은
검증하지 않았다. 기존 20-trial coding baseline도 아직 실행하지 않았다.

## 중요한 반례와 진단

1. 처음에는 workspace 신뢰·frontmatter 설정을 올바르게 반영하지 못했다. Agent 이름이 표시되는 것만으로
   실제 custom agent 적용을 판정할 수 없었다. 설정을 바로잡은 뒤 discovery를 다시 확인했다.
2. `init.tools`에는 custom agent를 선택해도 기본 도구 이름 60개가 나왔다. 모델의 별도 자기보고는
   `manage_task` 하나였으나, 자기보고는 완전한 capabilities audit가 아니다.
3. `--json-schema`를 포함한 진단 경로 두 개에서는 단일 JSON이 아닌 반복 응답이 있었고,
   한 경로에서는 `manage_task` 2개 step의 실행을 관찰했다. 설정 변화가 함께 있었으므로 schema flag
   하나만 원인이라고 확정하지 않는다. `status=SUCCESS`만으로 유효 제안을 판정하지 않았다.
4. 실제 xgen 연결은 해당 schema flag를 쓰지 않고, 단일 JSON 검사와 xgen의 원래 proposal 검증을 사용했다.
   따라서 **공식 custom agent 설정만으로 agy 하네스를 완전히 제거했다고 할 수 없다.**
5. 첫 연결기 실행은 xgen이 지원하지 않는 inference CLI flag를 사용해 세 사례 모두 argument 단계에서
   실패했다. 모델 I/O 전 오류였고, 지원하는 환경변수로 고친 뒤 다시 실행했다. 초기 기록을 보존했다.
6. 실행 뒤 연결기의 raw JSON 전달·중복 key/비유한값 거절을 강화했다. 원래 live proposal 9개 모두 strict
   JSON 오프라인 재검사를 통과했다. 이 보완 버전으로 새 live 실행을 했다고 주장하지 않는다.

## 사용자 요구와의 관계

이미 있는 것 `ALREADY_PRESENT`: xgen의 파일·process capability, 승인·journal·Receipt·재개 경로다.
이번 실제 호출은 이 기반을 그대로 사용했다.

확인한 후보 `ADOPT_WITH_GATES`: 공식 CLI를 제안 생성 경로로 쓰는 일반 transport다. 별도 API key로만
모델을 연결해야 한다는 제약은 이번 작은 실험에서는 피했다. 기능을 xgen 정식 provider로 추가한 것은 아니다.

현재 채택하지 않는 주장 `DO_NOT_ADOPT`: agy를 raw model처럼 완전히 통제했다는 주장, 모든 외부 도구·
private memory를 자동 이전했다는 주장, 별도 API와 동일한 요청·예산 계약을 보장한다는 주장이다.
기존 MCP·skills·memory 전체를 수집하거나 복사하는 작업은 하지 않았다.

후속 일반 설계에는 native tool 사용 시 처리, 유효 JSON·응답 크기·시간·동시성 한도, 숨은 retry/model-call
수, 출력 token 상한, usage/cache 의미, CLI/agent/settings 변경을 반영하는 profile·resume drift 계약이
필요하다. 현재 CLI 경유 경로로 기존 API 평가의 지출 guard와 공정 비교를 그대로 충족한다고 볼 수 없다.

소유자: agy의 도구 잔존·schema 출력·runtime 동작은 agy 경로, transport와 xgen 계약·승인·budget·Receipt
연결은 xgen 과제다. 특정 모델이나 과업의 답을 xgen 엔진에 넣는 수정은 하지 않았다.

## 근거

- [비민감 결과·원본 artifact hash](2026-11-22-agy-planner-probe-results.json).
- [공식 headless/stream-json/structured output](https://antigravity.google/docs/cli/headless/).
- [공식 custom agent 정의](https://antigravity.google/docs/subagents?app=cli).
- [공식 settings schema](https://antigravity.google/schemas/v1/settings.json).
- [custom agent discovery/model tier에 대한 maintainer 설명](https://github.com/google-antigravity/antigravity-cli/issues/1270).

로그인 값·계정·토큰·대화 ID·사용자 설정 원문은 결과 기록에 포함하지 않았다. 원본 trace와 임시 연결기는
작업 전용 비공개 임시 디렉터리에 두었다.
