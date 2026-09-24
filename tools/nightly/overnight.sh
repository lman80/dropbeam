#!/bin/bash
# Unattended overnight regression loop: real-app sends + Quick Sends across every
# pair, plus engine (lab) suites, repeated so intermittent failures surface.
# Everything appends to realapp.log / overnight.log in this folder.
cd ~/DropBeam-wt/nightly
LOG=overnight.log
round=0
end=$(( $(date +%s) + ${HOURS:-4} * 3600 ))
while [ "$(date +%s)" -lt "$end" ]; do
  round=$((round + 1))
  echo "=== overnight round $round $(date '+%F %T')" | tee -a $LOG realapp.log
  python3 realapp.py "m1>mac2,mac2>m1,lin>m1,lin>mac2" photo,multi,folder,many,video,big send >> $LOG 2>&1
  python3 realapp.py "m1>mac2,lin>m1,lin>mac2,mac2>m1" photo,folder,video quicksend >> $LOG 2>&1
  python3 realapp.py "m1>lin,mac2>lin" photo,multi,many send >> $LOG 2>&1
  for s in quick edge many; do ./run.sh ov$round-m1-mac2-$s mac2.addr auto $s >> $LOG 2>&1; done
  ssh penis "cd ~/nightly && ~/run.sh ov$round-lin-m1-full m1.addr auto full && ~/run.sh ov$round-lin-m1-edge m1.addr auto edge" >> $LOG 2>&1
  echo "=== round $round done $(date '+%T'): $(grep -c ' PASS ' $LOG) pass lines, $(grep -c -E ' FAIL | ERR ' $LOG) fail/err lines total" | tee -a $LOG
done
echo OVERNIGHT_DONE >> $LOG
