# A display of one capturing lambda is a fixed-size array of that closure's
# own type; the element calls through its subscript, reading `imm` captures,
# writing through `mut` captures, and keeping `var` snapshots.
def main():
    var k = 3
    var scaled = [lambda (x: Int) {k} -> Int: x * k]
    print(len(scaled))
    print(scaled[0](2))

    var pieces: List[Int] = [1]
    var extend = [lambda (x: Int) {mut pieces}: pieces.append(x)]
    extend[0](4)
    extend[0](5)
    print(len(pieces))

    var label = String("abc")
    var measure = [lambda (x: Int) {var label} -> Int: x + label.byte_length()]
    print(measure[0](1))
    print(measure[0](10))
