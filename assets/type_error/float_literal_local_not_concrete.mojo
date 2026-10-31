# expect: cannot infer type parameter 'value' of 'FloatLiteral'
# The bare `FloatLiteral` spelling binds its value per call only as a
# parameter annotation; on a local it is the struct with its parameter
# unbound. The pin infers a bare local annotation's parameters from the
# initializer and rejects this one as "cannot implicitly convert
# 'FloatLiteral[0.5]' value to 'FloatLiteral[_]'" (R1787).
def main():
    var x: FloatLiteral = 0.5
    print(Float64(x))
