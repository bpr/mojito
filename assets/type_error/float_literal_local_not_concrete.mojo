# expect: 'FloatLiteral[_]' is not concrete
# The bare `FloatLiteral` spelling is not concrete outside a parameter
# annotation, where its value would infer per call.
def main():
    var x: FloatLiteral = 0.5
    print(Float64(x))
