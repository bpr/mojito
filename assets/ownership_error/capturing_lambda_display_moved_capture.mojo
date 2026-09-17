# A display element's `imm` capture borrows its owner for as long as the
# display lives, so the owner cannot be transferred before the element call.
# expect: conflicts with live reference
def main():
    var label = String("abc")
    var measure = [lambda (x: Int) {label} -> Int: x + label.byte_length()]
    var moved = label^
    print(measure[0](1))
    print(moved)
