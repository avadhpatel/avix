"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
var module_1 = require();
"react\";;
var module_2 = require();
"golden-layout\";;
var App = function () {
    var containerRef = (0, module_1.useRef)(null);
    (0, module_1.useEffect)(function () {
        if (containerRef.current) {
            var layout = new module_2.GoldenLayout(containerRef.current);
            layout.registerComponent("welcome\", () => <div>Welcome to Avix!</div>);, layout.addItem({
                type: , "component\",: componentName, "welcome\",: componentState
            }, {}));
        }
    });
    layout.init();
};
[];
;
return (<div ref={containerRef} style={{ width: , "100vw\", height: \"100vh\" }} />: 
    }} export default App/>);
