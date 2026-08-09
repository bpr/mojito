# Current Mojo treats a bare `def(...)` collection-element type position as a
# trait, not a storable callable value.
# expect: 'List' element type
def double(x: Int) -> Int:
    return x * 2

def main():
    var fns: List[def(Int) -> Int] = [double]
    print((fns[0])(21))
