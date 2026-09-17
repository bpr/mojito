# A display of a capturing lambda cannot be reassigned from another display:
# the stored closure would move to an outer binding.
# expect: closures cannot escape
def main():
    var k = 3
    var fns = [lambda (x: Int) {k} -> Int: x * k]
    fns = [lambda (x: Int) {k} -> Int: x + k]
    print(fns[0](2))
