# expect: doesn't match expected origin 'ImmStaticOrigin'
# A tracked local place does not convert to a `ref[ImmStaticOrigin]` parameter.
def observe_static(ref[ImmStaticOrigin] value: Int):
    print(value)
def main():
    var value = 41
    observe_static(value)
