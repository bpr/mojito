def select[
    origins: OriginSet,
    //,
    enabled: Bool,
    callback: def(Int) capturing[origins] -> Int,
](value: Int) -> Int:
    comptime if enabled:
        return callback(value)
    else:
        return value


def main():
    var offset = 1

    @parameter
    def add(value: Int) -> Int:
        return value + offset

    print(select[True, add](41))
    print(select[False, add](42))
