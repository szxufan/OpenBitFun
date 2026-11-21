# Interactive figures, version 1

For the built-in components below, put one figure in the HTML fragment. Put its caption in a figcaption and its specification in a direct child script of type application/json. Escape literal less-than characters inside JSON strings as \u003c. The viewer interprets the specification and supplies the component’s interaction code.

For a custom diagram, ordinary HTML and CSS can provide interaction directly. For example, a details element can expand a component description. These fragments need no JSON specification. Use the shared theme variables `--background`, `--foreground`, `--muted-foreground`, `--border`, and `--blue` for drawing surfaces, strokes and accents. The host also supplies heading sizes and the `text-muted`, `text-small`, `btn`, and `viz-controls` classes.

    <figure data-openbitfun-visualize>
      <figcaption>Build time by platform (minutes)</figcaption>
      <script type="application/json">
      {"version":1,"kind":"bar","columns":["Build","Minutes","Platform"],"filterColumn":"Platform","rows":[["Baseline",8,"Linux"],["Release",6,"Linux"],["Baseline",10,"macOS"],["Release",9,"macOS"]],"followUp":"Explain the selected build time."}
      </script>
    </figure>

Column names are unique. Each row has one value per column. Numbers use JSON numbers and missing values use null. An optional filterColumn names a string column which is removed from the plotted dimensions. An optional followUp provides a question. A reader chooses a row and can put that question and the selected values in the composer. Keep the question relevant to the data.

| kind | Columns after filtering |
| --- | --- |
| line, area, bar | X label, then one or more numeric series. A missing value leaves a gap. Area and bar accept optional stacked of absolute or percent. Percent requires nonnegative observations. |
| sparkline, ranking | Label, value. Ranking sorts by value. |
| pie, funnel | Label, nonnegative value. |
| histogram | One numeric observation per row. The viewer computes bins. |
| waterfall | Label, signed movement. The first movement starts at zero. For an opening balance, use it as the first movement. Omit a redundant total row. |
| heatmap | X label, Y label, numeric intensity. |
| scatter | Numeric X and Y. |
| box-plot | Label, minimum, lower quartile, median, upper quartile, maximum. |
| sankey | Source name, target name, positive weight. Flows must be acyclic. |
| simulation | Parameter setting, then one or more computed outputs. The slider moves between the rows in supplied order. Compute the outcomes before writing and use meaningful sampling intervals. |
| table | One or more named columns. |
| calendar | Local start, local end, event title. Use YYYY-MM-DDTHH:mm for both times, converting the source times into a single stated time zone. The reader can navigate day, week and month. |
| request-waterfall | Request name, numeric start, numeric end in the same unit. Overlapping spans retain their actual offsets. |
| flame-chart | Semicolon-separated stack from caller to callee, positive sample weight. Repeated prefixes combine. |
| stack-trace | Function, source file, positive line number, and optionally a source excerpt. Supply frames in stack order. Selecting a frame reveals its location and excerpt. |

For a what-if calculation, sample a real parameter and record its output. For example, with fixed demand of 120 requests per minute and identical servers, these rows show the per-server load.

    <figure data-openbitfun-visualize>
      <figcaption>Load per server at 120 requests per minute</figcaption>
      <script type="application/json">
      {"version":1,"kind":"simulation","columns":["Servers","Requests per minute per server"],"rows":[[1,120],[2,60],[3,40],[4,30],[5,24],[6,20]]}
      </script>
    </figure>

## Standard chart configurations

The same figure accepts a named chart configuration as its JSON, or an object with a `charts` array containing two or three configurations with distinct IDs. Each chart has an ID, title, data rows and spec. Optional description, summary, height and showHeader control presentation. Supply object rows, or supply columns and aligned matrix rows. An empty dataset displays an empty view. The host binds the fields and mounts the chart. Include only the options needed for the answer.

    {"id":"release","title":"Duration per platform","rows":[{"release":"A","linux":20,"mac":30},{"release":"B","linux":12,"mac":18}],"spec":{"type":"bar","x":"release","y":["linux","mac"],"yLabel":"Seconds"}}

| Type | Bindings and behavior |
| --- | --- |
| line | x and y. Numeric x values sort in increasing order. Null y leaves a gap. Linear or log axes support explicit domains. |
| area, stackedArea | x and y. Stacked areas sum the visible measures. |
| bar, horizontalBar | x and y. Bind categories to the physical category axis. Use original, ascending or descending sortOrder. |
| stackedBar, horizontalStackedBar, stackedBar100, horizontalStackedBar100 | Supply wide measures or a series field with one observation per category and series. Hiding a series recalculates the percentage shares. |
| sparkline | x and y. Use a compact inline trend when the surrounding text explains its scale. |
| scatter | Numeric x and y with optional series. Use linear or quadratic fit for a separate least-squares curve per group. |
| pie | category and nonnegative value. Optional innerRadius is a fraction from zero up to one, excluding one. Supply centerValue with an optional centerLabel. |
| histogram | Raw observations use input raw and x. Reviewed bins use input binned, x containing lower, upper and label field names, and y holding counts. Proportional widths support frequency and density. |
| waterfall | x labels ordered changes in y. Supply beginning, optionally ending. The viewer checks reconciliation and draws both balances. |
| heatmap | x, y and value. Supply xOrder, yOrder or colorDomain where needed. Use null for a missing observation. |
| leaderboard, rankedList | category and value with optional sortOrder. Show more reveals every observation. |
| funnel | category and value in the original stage order. Keyboard inspection includes share of first and change from previous when denominators exist. |
| boxPlot | x groups observations of y. For reviewed summaries supply minimum, lowerQuartile, median, upperQuartile and maximum fields. Raw values use Tukey, minMax or percentile whiskers. |
| sankey | stages names two to four fields, value names the additive count. Submit one row per complete path. Shared links aggregate and repeated labels at different stages remain distinct. |

Cartesian measures support wide field lists or long data with a series field. For an interval use uncertainty with lower and upper field names and style bars or band. Use referenceLines, referenceBands and point annotations for measured context. Value formats accept decimal, percent (fractional inputs), and currency with a currency code. Use the compact version 1 shape for filterColumn or a selected-row followUp. Use a comparison for related charts drawn from one reviewed set of observations. An optional filterColumn on the comparison names a string field in every source row. The same selection updates all charts and restores on this device.

## Specialist components and panels

Supply version 2 and one of the following kinds in the figure. These widgets keep their reader state on the viewing device. Use milliseconds for traces and chronological order for CPU samples. Strings and source excerpts are displayed as text.

### Illustrated introductions

Use `kind: "tabs"` with a caption naming the subject and a short `description`. Each panel answers one question. Use a single panel for a single perspective. The renderer provides typography, active pills, directional connectors, responsive layout, and a theme-aware line illustration palette.

- A panel's `nodes` array has two to five stages. Each has a title, optional short caption, and an optional icon: desktop, runtime, artifacts, phone, relay, server or workspace. For other subjects, draw the relevant object using `drawing`, an array of SVG path objects in a 146 × 95 view box. Each path has `d` and a tone of face, side, inset, accent or solid. Use one accent to point to the meaningful part of the drawing.
- A panel's `variants` array adds selectable alternatives. Each entry has a label and can supply steps (strings), a detail sentence, or replacement nodes. Use fixed panel nodes for shared structure and changing steps for modes, or variant-specific nodes to show a change in the route itself.
- A panel's `outline` shows ordered layers or milestones. Each row has title, summary, optional path and optional boolean highlight. Use one language for the labels and keep each caption to a phrase. Pick representative modules instead of listing every crate.

    {"version":2,"kind":"tabs","description":"An editor turns a draft into a reviewed change.","panels":[{"title":"Workflow","nodes":[{"icon":"desktop","title":"Open a draft","caption":"Start in the editor"},{"icon":"runtime","title":"Review","caption":"Run the checks"},{"icon":"artifacts","title":"Result","caption":"A reviewed change"}],"variants":[{"label":"Quick","steps":["Edit","Check"],"detail":"For a small change."},{"label":"Full","steps":["Plan","Edit","Review"],"detail":"For a change across modules."}]},{"title":"Architecture","outline":[{"title":"Interface","path":"src/app","summary":"Receives edits."},{"title":"Engine","path":"src/core","summary":"Evaluates changes.","highlight":true}]}]}

- **layers** accepts 2–12 ordered layers. Each has a short name and one-sentence summary, with optional path and modules (an array of representative names). Order the layers as they appear in the system and explain the dependency direction in the optional description. Put each responsibility and its representative modules in the selected layer’s detail. Use this single figure for a layered overview.
- **calendar** accepts an anchor date, optional view (day, week or month), start and end visible times, now, timeZone, emptyLabel and an events array. Events have title, start and end, with optional date, id, tone, detail and video. Times are local HH:mm and may end at 24:00. Convert timestamps to the displayed zone before writing.
- **request-waterfall** accepts spans with unique id, name, startMs and durationMs. Optional parentId, service, status and scalar attributes supply detail. A supplied durationMs must cover all spans. Provide criticalPath only when the trace records it. The reader can select, search, collapse, sort, zoom and pan.
- **flame-chart** accepts frames with id, name and optional category. Each chronological sample has a root-to-leaf stack of IDs and a positive durationMs. Use an empty stack for idle time. A returning function after an idle interval gets its own call. The function table reports inclusive and self time.
- **stack-trace** accepts error with name, message and optional raw stack. Each frame has name, optional id, file, line, column, kind (app or runtime), async and source containing startLine and lines. Add causes with their own error and frames. Raw stack text is displayed for causes with no frames.
- **tabs** accepts panels with title and optional text, columns and rows, progress containing label, value and max, fields and followUp. A field with label and options renders a select, or supply type radio. Text and checkbox fields accept an initial value. Number and range fields require min, max and value and accept a positive step. A selected tab can hold a card, table, progress indicator and a small form. The follow-up button puts the selected field values into the current composer.

    {"version":2,"kind":"layers","description":"Dependencies point down the stack.","layers":[{"name":"Application","summary":"Receives requests and coordinates operations.","path":"src/app","modules":["router","handlers"]},{"name":"Domain","summary":"Defines the rules for placing an order.","path":"src/domain","modules":["orders"]}]}

    {"version":2,"kind":"request-waterfall","spans":[{"id":"get","name":"GET /orders","startMs":0,"durationMs":120},{"id":"db","parentId":"get","name":"Read orders","startMs":10,"durationMs":80},{"id":"cache","parentId":"get","name":"Read cache","startMs":20,"durationMs":15}]}

Here the children overlap. The request lasts 120 ms, of which 40 ms is direct time.

    {"version":2,"kind":"flame-chart","frames":[{"id":"render","name":"Render"}],"samples":[{"stack":["render"],"durationMs":3},{"stack":[],"durationMs":1},{"stack":["render"],"durationMs":2}]}

This profile has two separate render calls starting at 0 ms and 4 ms. Keep the idle sample to preserve the timeline.
