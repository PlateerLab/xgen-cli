# ADR-0091: 도구 collection의 다른 언어 summary 보강

- 날짜: 2026-11-28
- 상태: 채택, 구현·로컬 검증 완료
- 범위: `xgen tools enrich` 명령, worker의 보강 계획·검증·저장, 검색 결과의 설명 표시, 언어 힌트 생략.
  Run의 모델 요청 형식·권한·journal은 바꾸지 않는다.

## 근거

교차 언어 요청(한국어 요청 x 영어 명세, 영어 검색어 x 한국어 명세)은 검색 누락이 컸다. 운영 중인 검색어 언어
힌트(Q)는 모델이 자주 무시했다. 사전 등록한 두 비교에서 수집 시점 summary 보강(S)이 Q보다 나았다.

| 데이터 | 검색어 모델 | Q hit@5 | S hit@5 | Q hit@1 | S hit@1 |
|---|---|---|---|---|---|
| 2026-10-13 held-out | deepseek-flash | 80 | 87 | 37 | 66 |
| 2026-10-13 데이터 재실행 | agy Flash Low | 89 | 94 | 45 | 63 |
| 2026-11-28 held-out(판정) | agy Flash Low | 98 | 104 | 66 | 76 |

11-28 판정 기준(hit@5 +4 이상, 칸 하락 3 미만, hit@1 하락 없음, 보강 실패 2% 이하, 생성 실패 증가 없음)을
모두 통과했다. 같은 문자 체계 칸도 올랐다(45 -> 49). 색인되지 않는 description·파라미터 정보가 summary로
옮겨지는 효과가 섞여 있다.

## 결정

1. 기본은 꺼 둔다. 사용자가 `xgen tools enrich --name <이름> --languages <목록>`을 실행할 때만 모델을 부른다.
   operation 정보가 모델 프로필로 나가므로 명시적 명령이 그 동의다. 프로필은 활성 프로필 또는 `--profile`.
2. 생성 문장은 측정한 방식대로 원본 summary 뒤에 ` / `로 붙인 입력으로 그래프를 빌드한다. 빌드 뒤 tool
   metadata(`ai_metadata.one_line_summary`)나 description에 넣는 방식은 같은 문장으로도 세 묶음 hit@1 합이
   205 -> 182, 205 -> 161로 떨어져 채택하지 않았다.
3. 모델이 보는 설명은 원본이다. worker는 원본 빌드의 tool 설명을 기록하고 search 후보·describe에서 그것을
   돌려준다. 실행용 원본 명세 사본(`xgen_http_sources`)과 입력 digest는 원본 그대로 둔다.
4. 보강 기록은 artifact에 넣고 rebuild 때 다시 적용한다. 모델을 다시 부르지 않는다.
5. 보강한 언어의 문자 체계로 쓴 요청에는 검색어 언어 힌트를 넣지 않는다. 판정한 조건 S가 힌트 없음이다.
6. 모델 호출은 Rust host가 하고, worker는 요청 묶음(20개, 누락 언어 조합별)과 strict schema를 만들고 응답을
   검증한다(언어별 문자 체계, 200자 이하). 실패 operation은 한 번 다시 묻는다. 측정한 생성 문구와 같다.
7. 모델 호출은 planner와 같은 endpoint·전송 보안·request option을 쓰는 `OpenAiJsonTask`로 한다.
   agy 프로필은 같은 고정 CLI를 계획 제안 대신 JSON 작업 마감 문구로 쓴다.

## 검증과 한계

- worker 단위: 한국어 명세·영어 명세 두 경우에서 보강 후 다른 언어 검색어가 맞는 도구를 찾고, 후보·describe
  설명은 원본이며, 언어 힌트가 생략되는 것. 잘못된 문자 체계·200자 초과·누락 key 거절과 재시도, rebuild 유지.
- CLI: 가짜 OpenAI 호환 서버로 strict schema 이름, 누락 언어만 요청, 사용량 합계, list 표시를 확인.
- 실제 모델: 11-28 held-out 명세 4개를 agy Flash Low로 보강(287 operation, 실패 0, 15회 호출). 측정
  검색어로 hit@5 95 -> 103, hit@1 59 -> 75. 측정 때 생성 문장을 그대로 넣은 빌드는 측정 S와 앞 5개 16/16 일치.
- agy로 쉬다가 처음 부른 실행에서 모든 호출이 `unavailable`이었고 바로 다시 실행하면 성공했다(3회 관찰).
  원인은 agy 자동 업데이트였다(2026-11-28 확인, [agy provider](../development/agy-provider.md#자동-업데이트)).
  실패 분류는 출력(`model_failures`)과 agy 진단 줄(`XGEN_AGY code=agy_exit|pin_changed`)로 남기고, 재시도 전 10초 쉰다.
- HTTPS로 import한 collection도 지원한다(2026-11-28 후속). 보강할 때 import와 같은 로더로 다시 받아 오고, 출처 정보는 원본 빌드 값으로 되돌린다. 실제 Qdrant URL 1건에서 보강 전 결과가 측정 C와 같았다(hit@5 26).
- 사용량(2026-11-28 후속): 보강은 Run이 아니라 collection 작업이라 Run 사용량 기록(`xgen usage <run>`)에 넣지 않고,
  보강 기록에 `usage`(모델 호출 수, 실패 분류별 횟수, 입력·출력 token, 모델 대기 시간)로 함께 저장한다. 그 문장을
  만든 비용이 문장과 같은 곳에 있으므로 `tools list`가 계속 보여 주고 rebuild도 유지한다. token은 성공한 호출에서
  provider가 알려 준 값의 합이며 실패한 호출의 소비는 알 수 없다. 이전 기록에는 `usage`가 없다.
- held-out은 모두 소진됐다. 다음 효과 측정은 새 held-out이 필요하다.
