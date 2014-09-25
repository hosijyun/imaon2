var fs = require('fs');
var PEG = require('pegjs');


var stuff = fs.readFileSync(process.argv[2], 'utf-8');
stuff = stuff.substr(stuff.indexOf('------------- Defs -----------------\n') + 37);

var peg = fs.readFileSync(__dirname + '/untable.peg', 'utf-8');
//console.log(PEG.buildParser(peg, {output: 'source'}));
var parser = PEG.buildParser(peg);
try {
    //stuff = '[1, 2, 3, 4, 5]';
    var result = parser.parse(stuff);
} catch(e) {
    console.log('At line ' + e.line + ':');
    console.log(e.message);
}

// Some pre-filtering
var instructions = result.filter(function(defn) {
    return (
        defn.supers.indexOf('Instruction') != -1 &&
        defn.props.Opcode &&
        defn.props.Namespace != 'TargetOpcode' &&
        !defn.props.isPseudo &&
        !defn.props.isAsmParserOnly &&
        !defn.props.isCodeGenOnly
    );
}).map(function(defn) {
    var predicates = {};
    (defn.props.Predicates || []).forEach(function(p) {
        predicates[p] = true;
    });
    return {
        name: defn.name,
        inst: defn.props.Opcode,
        pattern: defn.props.Pattern,
        asm_string: defn.props.AsmString,
        predicates: predicates,
        namespace: defn.props.Namespace,
        decoderNamespace: defn.props.DecoderNamespace,
    };
});
var patternOperators = {};
result.filter(function(defn) {
    return (
        defn.supers.indexOf('SDPatternOperator') != -1 &&
        defn.supers.indexOf('PatFrag') != -1 &&
        !defn.props.PredicateCode &&
        !defn.props.ImmediateCode
    );
}).forEach(function(defn) {
    patternOperators[defn.name] = {
        operands: defn.props.Operands,
        fragment: defn.props.Fragment,
    };
});
var output = {
    instructions: instructions,
    patternOperators: patternOperators
};
fs.writeFileSync(process.argv[3], JSON.stringify(output));
