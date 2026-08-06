10 PRINT "Math AI Coach"
20 PRINT "============="
30 INPUT "Which grade are you? (1–6) ", gr
40 PROMPT "Ask a simple math problem for " + STR$(gr) + " grade elementry student."
50 INPUT "Answer : "; an$
60 PROMPT "Evaluate user's answer: " + an$
70 INPUT "Continue? (1=yes, 2=No) "; ct
80 IF ct = 1 THEN 40
90 PRINT "Bye"
100 END