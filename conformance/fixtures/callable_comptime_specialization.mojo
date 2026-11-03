def select[
    enabled: Bool, F: def(Int) -> Int
](callback: F, value: Int) -> Int:
    comptime if enabled:
        return callback(value)
    else:
        return value


def main():
    var offset = 1

    def add(value: Int) {imm offset} -> Int:
        return value + offset

    print(select[True](add, 41))
    print(select[False](add, 42))
