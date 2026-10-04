# 최종 응답과 실제 변경 비교

이 평가는 일반적인 재현·측정 도구다. 도메인 예제와 기대값은 `cases.json`에만 두고 engine이나 공유 prompt를
변경하지 않는다. 현재 corpus는 Python 함수 네 종류이며 각 사례의 독립 unittest가 먼저 실패하도록 구성했다.

## 실행

먼저 TTY에서 `xgen`을 실행해 사용할 모델 프로필을 저장한다. 평가는 해당 active profile과 OS credential store를
사용한다. 별도 key file이나 argv token을 받지 않으며 `XGEN_*`, `XGENY_*`, `DEEPSEEK_*` override는 제거한다.

```bash
python3 scripts/evaluate-final-responses.py --split design --attempts 3
python3 scripts/evaluate-final-responses.py --split held_out --attempts 3
python3 scripts/evaluate-final-responses.py --claim-style narrative --attempts 3
```

기본 `literal`은 최종 함수의 코드 블록을 요청하고 `narrative`는 일반적인 변경 설명만 요청한다. `--binary`로
별도 빌드를 선택할 수 있다. 각 실행은 독립적인 임시 workspace/state를 만들고 API를 실제 호출한다. fixture
수정과 테스트 실행은 해당 REPL process에서 명시적으로 허용한다. 독립 테스트 process에는 최소 환경만 전달한다.
고객 코드나 credential을 fixture에 넣지 않는다.

## 결과와 해석

마지막 `REPORT` 경로에 private 임시 결과가 남는다. 각 사례의 `answer.txt`, `outputs.json`, `tests.txt`, 실제
workspace와 `result.json`을 대조한다. `report.json`은 binary·corpus digest를, `cases.json`은 사용한 corpus를 보존한다.
모델 ID와 request-profile digest도 기록한다. 기존 Run projection과 journal·sidecar에서 결과를 읽으며 stdout의
자연어를 완료 증거로 취급하지 않는다.

- `task_completed`: durable candidate가 작업 완료 종류인가.
- `tests_passed`, `tests_unchanged`: 모델과 별도로 실행한 테스트가 통과했고 허용한 source 외의 모든 fixture 파일 bytes가 그대로인가.
- `model_calls`, `tool_effects`: journal의 possible-send reservation과 committed 성공 effect 수. Process의
  비정상 exit도 실행 effect 자체는 성공적으로 기록될 수 있으므로, 이 숫자는 테스트 통과 횟수가 아니다.
- `usage`: `xgen usage`로 조회한 provider-reported 토큰·cache·HTTP 시간과 관측 coverage다.
  누락된 호출은 추정하지 않으며 이전 binary에서 command가 없으면 `null`이다. 가격 단가를 주지 않으므로
  금액은 계산하지 않는다. [계측 계약](../../docs/development/model-usage-2026-10-04.md)을 따른다.
- `final_content_observed_after_write`: 마지막 대상 파일의 native write/apply-patch 뒤 read-text 관찰이 최종 파일 내용과 같은가. Native write가 없으면 `null`이며 process가 임의로 쓴 파일의 순서는 추론하지 않는다.
- `quote_status`: standard Python AST로 이름이 같은 함수·명시적 return 인용을 비교한 결과다. `match`는 인용
  일치를 뜻하며 자연어 전체의 사실성을 보장하지 않는다. `mixed_quotes`는 일치·불일치 인용이 함께 있어서
  검토가 필요하다. 정상적인 수정 전·후 설명도 이 상태가 될 수 있다. `needs_review` 역시 거짓 주장 판정이
  아니며 문맥을 사람이 확인해야 한다. `not_claimed`는 narrative에 비교할 함수·명시적 return 인용이 없다는 뜻이다. `return` 없는 식이나 비정형 설명은 자동 비교하지 않는다.
  `missing_claim`은 literal 요청의 함수 코드가 없고, `invalid_source`는 파일의 Python 문법이 잘못된 경우다.

Exit 0은 전체 사례의 실행·독립 테스트·테스트 파일 보존 조건이 충족됐다는 뜻이며 답변의 사실성 인증이 아니다.
조건이 실패하면 Exit 1이다. API 없이 evaluator 자체를 검증하는 테스트만 CI에서 실행한다.

```bash
python3 -m unittest discover -s scripts/tests -v
```

설계 중 확인한 사례로 원인을 찾고, engine 변경이 필요하면 새로운 별도 검증 사례에서 효과를 확인한다. 이미
검토한 held-out 사례를 이후 변경의 설계에 사용하면 더 이상 held-out으로 계산하지 않는다.
