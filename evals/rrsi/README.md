# RRSI 방식의 첫 비교 실험

일반적인 작업 프롬프트 변경 하나를 기존 하네스와 비교하는 pilot이다. RRSI 논문의 전체 자동 개선 시스템을
재현하지 않는다. 모델·engine·공유 system prompt는 고정하고, 실험 후보를 작업 요청에 추가한다.

## 가설과 후보

가설: 최종 응답 전에 수정 파일을 read-text로 다시 관찰하면 최종 함수 인용과 실제 파일의 일치가 개선된다.
후보는 `readback.txt`의 일반적인 지시 하나다. 함수 이름·fixture 값·해결 코드를 포함하지 않는다.

설계 사례는 빈도 계산과 범위 병합, 별도 검증 사례는 quoted field parsing과 sliding window다. 기존 최종 응답
평가에서 검토한 네 사례는 재사용하지 않는다. 후보와 corpus를 실행 전에 고정하고 SHA-256을 기록한다.

## 실행

저장된 active model profile과 OS credential store가 필요하다. 실제 API 호출을 수행한다.

```bash
python3 scripts/experiment-rrsi.py --binary target/release/xgen --attempts 2
python3 -m unittest discover -s scripts/tests -v
```

설계 평가 뒤 별도 검증 평가를 실행한다. 각 사례·variant를 독립 workspace/Run에서 실행하고, baseline/candidate
순서는 사례와 반복 번호에 따라 번갈아 배치한다. 기본값은 네 사례 × 두 variant × 두 반복, 총 16회다.
입력 파일이나 binary가 실행 중 변경되면 결과를 확정하지 않는다. timeout은 자동 재시도하지 않는다.

`--cases`, `--candidate`, `--attempts`, `--timeout`으로 별도 실험을 구성할 수 있다. 모델이 실제로 사용한
request-profile digest가 모든 실행에서 같아야 비교가 유효하다. 외부 fixture는 신뢰할 수 있는 synthetic
자료만 사용한다. 프로세스 실행을 포함하므로 OS sandbox로 간주하지 않는다.

## 측정과 채택 규칙

- 모델과 독립적으로 테스트를 실행하고, 허용한 source 외의 fixture bytes가 보존됐는지 확인한다.
- 최종 응답의 함수 인용은 실제 파일과 AST로 비교한다. 수정 전·후 인용이 섞이면 수동 검토가 필요하다.
- model call reservation, 성공 tool effect, CLI 소요 시간, 수정 뒤 최종 내용의 native read-text 관찰을 기록한다.
- 설계 baseline 실패가 서로 다른 두 사례에서 재현되고, 후보의 정확한 인용 성공 수가 늘어야 한다.
- 작업 성공 및 별도 검증 인용 결과가 나빠지면 거절한다. split별 평균 model calls 증가는 25% 이하여야 한다.
- 통과해도 `promising_requires_larger_validation`이다. 작은 표본만으로 engine 변경을 자동 채택하지 않는다.

토큰과 금액은 현재 journal에서 수집할 수 없다. 호출 수·시간은 비용의 대용 지표이며 실제 API 비용이나
통계적 유의성을 뜻하지 않는다. 정확한 코드 인용은 자연어 전체의 사실성 인증이 아니다. 별도 검증 사례를
결과 분석에 사용한 이후에는 다음 후보의 untouched held-out으로 재사용하지 않는다.

각 실험의 `registration.json`, `ledger.jsonl`, `report.json`과 synthetic workspace는 출력된 임시 경로에
보존된다. fixture 내 모델 답변이나 발견한 코드와 독립 테스트를 대조할 수 있다.
