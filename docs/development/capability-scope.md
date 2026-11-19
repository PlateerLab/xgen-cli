# Run별 capability 범위

`run --capability ID`로 계획과 실행에 등록할 capability ID를 제한해. 여러 번 지정할 수 있어.
옵션을 생략하면 기존 전체 catalog를 유지해. 특정 업무나 모델에 종속되지 않는 일반 기능이야.

```sh
xgen run --workspace . --allow-dir . --allow-read --allow-remote-model-egress \
  --capability xgeny.fs/read-text 'README.md를 읽고 요약해'
```

선택은 원래 등록 가능한 정의와 instance의 부분집합이야. 모르는 ID·중복 ID는 Run을 만들거나
모델에 요청하기 전에 거절해. capability를 선택해도 파일·HTTP·실행 승인 조건은 그대로 필요해.
모델이 목록 밖의 작업을 제안하면 admission에서 거절해.

정렬된 목록을 manifest, authority, 실행 profile digest에 결속해. `resume`은 저장된 범위를
복원하고 새 범위 옵션을 받지 않아. 기본값은 manifest에 새 필드를 저장하지 않아 이전 Run과
전체 catalog 경로를 유지해. 도구 반환값·Receipt·원문은 수정하거나 요약하지 않아.

HTTP read만 선택한 호스트는 `tools describe`로 확인한 정확한 요청·contractDigest를 goal에
전달할 수 있어. describe가 범위 밖이고 계약도 없다면 에이전트는 선행조건 부족을 보고해야 해.
범위 선택은 부족한 계약을 추측하거나 HTTP 권한을 부여하는 기능이 아니야.

선택 범위에 맞춰 workspace·process·검색·describe 안내도 제한해. search가 없으면 검색어 언어 힌트를
조회하지 않아. 필수 완료 검사에 process.execute가 필요한데 이를 제외하면 모델 호출 전에 거절해.
기존 write/read 승인·credential·연결 drift 검사는 그대로 적용해.

검증은 [ADR-0087](../adr/0087-run-capability-scope.md)을 참고해. read/stat 승인 후 재개·선택 write의
별도 승인·범위 밖 write 차단과 shipment/sensor의 HTTP 입력·본문 동일성, 요청 크기 감소를 확인해.
실제 모델의 판단 품질 향상·토큰 과금 절감은 측정하지 않았어.

검증 결과: Rust798개 통과(4 ignored), Python162개 검사 통과(9 skipped), Clippy·fmt·문서·workflow
검사 통과. 기존 rc.6 바이너리에서 시작한 범위 미지정 Run의 재개도 두 HTTP fixture에서 확인했어.
이 기능은 rc.6 배포 이후의 main 변경이야.
