# A `def` keyed on a type and a scalar value parameter, with no compile-time
# control flow, derives its instances from the template the abstract check
# inferred: the elaborator folds the value to each instance's literal, which
# keeps the name's identity and takes a literal's facts (read, accumulated,
# lent to a read parameter, or folded with other literals into one).
def bump(x: Int) -> Int:
    return x + 1


def scaled[T: ImplicitlyCopyable & Deinitable, n: Int](x: T, base: Int) -> Int:
    var kept = x
    var acc = base * n
    acc += bump(n)
    acc += n * 10 + 1
    return acc


def flagged[T: ImplicitlyCopyable & Deinitable, on: Bool](x: T) -> Bool:
    var kept = x
    var seen = on
    return seen


def main():
    print(scaled[Int, 3](7, 2), scaled[String, 1]("s", 5))
    print(flagged[Int, True](4), flagged[String, False]("k"))
