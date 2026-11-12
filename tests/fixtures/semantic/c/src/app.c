// Межфайловая проверка — 雪
#include "ledger.h"
Count ledger_process(Ledger *ledger) {
    ledger->available = bump(ledger->available);
    return ledger->available;
}
