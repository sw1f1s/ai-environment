#ifndef AI_ENV_LEDGER_H
#define AI_ENV_LEDGER_H
typedef unsigned int Count;
typedef struct Ledger {
    Count available;
} Ledger;
Count bump(Count value);
Count ledger_process(Ledger *ledger);
#endif
