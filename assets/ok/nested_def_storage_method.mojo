# Capture-list nested `def`s whose lifted bodies take their closure's
# storage struct as `self`: every call passes the one struct, a moved
# register-passable capture is written in that call's own copy, and a
# moved memory-only one in place.
def twice(n: Int) -> Int:
    var y = n
    def get() {imm y} -> Int:
        return y + 1
    return get() + get()

def moved_scalar(n: Int) -> Int:
    var x = n
    def inc() {var x^} -> Int:
        x += 1
        return x
    var a = inc()
    return a * 100 + inc()

def moved_string(var s: String) -> Int:
    def more() {var s^} -> Int:
        s += "!"
        return s.byte_length()
    var a = more()
    return a * 100 + more()

def snapshot(n: Int) -> Int:
    var x = n
    def read() {var x} -> Int:
        return x
    x = 100
    return read() + x

def written(n: Int) -> Int:
    var total = n
    var step = 3
    def bump() {mut total, imm step}:
        total += step
    bump()
    bump()
    return total

def mixed(var label: String, k: Int) -> Int:
    def measure(extra: Int) {var label, imm k} -> Int:
        return label.byte_length() + k + extra
    return measure(1) * 100 + measure(2)

def main():
    print(twice(4))
    print(moved_scalar(4))
    print(moved_string("ab"))
    print(snapshot(5))
    print(written(1))
    print(mixed("abc", 10))
