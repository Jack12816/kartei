// A sample JSX component library for the extraction tests.

export default class Widget {
  constructor(props) {
    this.props = props;
  }

  get title() {
    return this.props.title;
  }

  set title(value) {
    this.props.title = value;
  }

  static create(props) {
    return new Widget(props);
  }

  render() {
    return <div className="widget">{this.props.title}</div>;
  }
}

const Header = ({ title }) => <h1>{title}</h1>;

const legacy = function (value) {
  return value * 2;
};

export async function fetchWidgets(url) {
  const response = await fetch(url);
  return response.json();
}

function* widgetIds() {
  yield 1;
}

export const VERSION = '1.2.3';

const DEFAULTS = { title: 'Untitled' };
