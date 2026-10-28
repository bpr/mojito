# expect: 'IntLiteral[_]' is not concrete
# The bare `IntLiteral` spelling is not concrete outside a parameter
# annotation, where its value would infer per call.
def main():
    var x: IntLiteral = 3
    print(Int(x))
