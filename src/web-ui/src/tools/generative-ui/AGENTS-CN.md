**中文** | [English](AGENTS.md)

# GenUI 历史查看器

FlowChat 在这里展示已保存的 GenUI 工具结果，并读取旧宿主的原生源码流。显示这些记录时才加载 OpenUI 解释器。新回复使用内置 visualize 技能和 `src/web-ui/src/tools/visualize`。长期架构事实维护在[现有架构文档](../../../../../docs/architecture/generative-ui-architecture.md)。

- 保留原应用标识、组件库版本、源码与初始状态。直接调用和延迟调用都从原会话与设备读取记录。未知格式提供原始内容查看，并保留数据。
- 历史视图只读。打开记录不能发起修复轮次、调用宿主服务、写入修改或执行文档脚本。
- 使用 OpenUI lang-core 和 react-lang 解析已存程序。指定版本的库负责 schema 与可信渲染器。按照顺序号和重置标记消费原生 GenUIStream，工具参数片段单独处理。
- 静态 HTML/CSS/SVG 使用不透明来源沙箱和结构化清理。历史宿主不提供脚本桥、原生 API、凭据、文件系统或网络访问。复用静态查看器时保留这些限制。
- 第三方来源及许可证记录在 components/vendor/README.md。
- 调整全局选择器时保留隐藏子项和嵌套堆叠的间距。热点调整用原生样式失效数据与性能记录验证。

[英文版](AGENTS.md)列出了组件契约、旧配置往返、Web UI 和浏览器的具体验收命令。Chrome 用例验证本机回放；远程传输需要在真实宿主间另行验证。
