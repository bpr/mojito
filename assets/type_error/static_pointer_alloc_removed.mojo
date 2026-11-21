# The pin's `Pointer` declares no static allocation; user code allocates
# through std.memory.
# expect: 'Pointer[Int, _]' value has no attribute 'alloc'
def main():
    var p = Pointer[Int].alloc(1)
