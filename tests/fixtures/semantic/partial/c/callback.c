unsigned call_unknown(unsigned (*callback)(unsigned), unsigned value) {
    return callback(value);
}
